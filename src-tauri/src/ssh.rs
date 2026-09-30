//! SSH local forwarding for database connections.
//!
//! `known_hosts` uses one DBSUAL-owned entry per line:
//! `<host> <port> <SHA256:base64-fingerprint>`.
//! Obtain the fingerprint out of band and add it explicitly before connecting.
//! OpenSSH hashed host entries and automatic trust-on-first-use are intentionally
//! unsupported.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
};

use russh::keys::ssh_key::{HashAlg, PublicKey};
use russh::{
    client::{self, Handle},
    keys::{agent::client::AgentClient, load_secret_key, PrivateKeyWithHashAlg},
    Disconnect,
};
use tokio::{
    io::copy_bidirectional,
    net::TcpListener,
    sync::{oneshot, Mutex},
    task::{JoinHandle, JoinSet},
};

const GENERIC_ERROR: &str = "No se pudo establecer el túnel SSH.";
static KNOWN_HOSTS_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatabaseTls {
    Disabled,
    Enabled,
}

#[derive(Clone, Debug)]
pub struct SshEndpoint {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub database_host: String,
    pub database_port: u16,
    pub known_hosts_path: PathBuf,
}

/// Secrets are deliberately not `Debug`, serializable, or cloneable.
pub enum SshAuth {
    Password(String),
    PrivateKey {
        path: PathBuf,
        passphrase: Option<String>,
    },
    /// Windows OpenSSH agent pipe, usually `\\.\pipe\openssh-ssh-agent`.
    WindowsAgent {
        pipe: PathBuf,
    },
}

#[derive(Debug)]
pub enum SshError {
    /// New key requiring a separate, explicit approval flow. This module does
    /// not write or approve it. The key and fingerprint are public material.
    UnknownHost {
        fingerprint: String,
        public_key: String,
    },
    /// A previously trusted SSH host presented a different key. It is blocked
    /// until handled by an explicit key rotation workflow.
    ChangedHost {
        fingerprint: String,
        public_key: String,
    },
    AuthenticationRejected,
    AgentUnavailable,
    AgentIdentityRejected,
    Failed,
}

impl std::fmt::Display for SshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownHost { .. } => {
                f.write_str("La clave SSH del servidor requiere aprobación explícita.")
            }
            Self::ChangedHost { .. } => {
                f.write_str("La clave SSH del servidor cambió y la conexión fue bloqueada.")
            }
            Self::AuthenticationRejected => {
                f.write_str("El servidor SSH rechazó las credenciales configuradas.")
            }
            Self::AgentUnavailable => {
                f.write_str("No se pudo acceder al agente OpenSSH de Windows.")
            }
            Self::AgentIdentityRejected => {
                f.write_str("El agente OpenSSH no ofrece una identidad aceptada por el servidor.")
            }
            Self::Failed => f.write_str(GENERIC_ERROR),
        }
    }
}

impl std::error::Error for SshError {}

impl SshError {
    fn failed() -> Self {
        Self::Failed
    }
}

#[derive(Default)]
struct HostKeyState {
    rejection: Option<SshError>,
}

struct HostVerifier {
    known_fingerprints: Vec<String>,
    host_is_known: bool,
    state: Arc<std::sync::Mutex<HostKeyState>>,
}

impl client::Handler for HostVerifier {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // Host certificates need CA/principal/validity policy. Reject them
        // until DBSUAL implements that policy instead of trusting only the
        // certificate's subject key.
        let russh::keys::PublicKeyOrCertificate::PublicKey {
            key: public_key, ..
        } = key
        else {
            if let Ok(mut state) = self.state.lock() {
                state.rejection = Some(SshError::failed());
            }
            return Ok(false);
        };
        let fingerprint = public_key.fingerprint(HashAlg::Sha256).to_string();
        let accepted = self
            .known_fingerprints
            .iter()
            .any(|known| known == &fingerprint);
        if !accepted {
            let rejection = if self.host_is_known {
                SshError::ChangedHost {
                    fingerprint,
                    public_key: public_key
                        .to_openssh()
                        .unwrap_or_else(|_| "unavailable".into()),
                }
            } else {
                SshError::UnknownHost {
                    fingerprint,
                    public_key: public_key
                        .to_openssh()
                        .unwrap_or_else(|_| "unavailable".into()),
                }
            };
            if let Ok(mut state) = self.state.lock() {
                state.rejection = Some(rejection);
            }
        }
        Ok(accepted)
    }
}

fn load_known_hosts(path: &Path, host: &str, port: u16) -> Result<(Vec<String>, bool), SshError> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Vec::new(), false))
        }
        Err(_) => return Err(SshError::failed()),
    };
    let mut fingerprints = Vec::new();
    for line in contents.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let (Some(entry_host), Some(entry_port), Some(fingerprint), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if entry_host == host && entry_port.parse::<u16>().ok() == Some(port) {
            fingerprints.push(fingerprint.to_owned());
        }
    }
    let host_is_known = !fingerprints.is_empty();
    Ok((fingerprints, host_is_known))
}

/// Record a key after the caller has obtained explicit user approval.
/// `public_key` must be one OpenSSH public-key line (`ssh-ed25519 AAAA... [comment]`).
/// Existing host entries are never silently replaced: same key is idempotent,
/// different key returns `ChangedHost` for a separate rotation decision.
pub fn record_known_host(
    path: &Path,
    host: &str,
    port: u16,
    public_key: &str,
) -> Result<String, SshError> {
    if host.is_empty() || host.chars().any(char::is_whitespace) || port == 0 {
        return Err(SshError::failed());
    }
    let key = PublicKey::from_openssh(public_key).map_err(|_| SshError::failed())?;
    let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
    let _guard = KNOWN_HOSTS_WRITE_LOCK
        .lock()
        .map_err(|_| SshError::failed())?;

    let current = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => return Err(SshError::failed()),
    };
    let mut host_was_found = false;
    for line in current
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let mut fields = line.split_whitespace();
        if let (Some(entry_host), Some(entry_port), Some(entry_fp), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        {
            if entry_host == host && entry_port.parse::<u16>().ok() == Some(port) {
                host_was_found = true;
                if entry_fp != fingerprint {
                    return Err(SshError::ChangedHost {
                        fingerprint,
                        public_key: key.to_openssh().map_err(|_| SshError::failed())?,
                    });
                }
            }
        }
    }
    if host_was_found {
        return Ok(fingerprint);
    }

    let mut updated = current;
    if !updated.is_empty() && !updated.ends_with(['\n', '\r']) {
        updated.push('\n');
    }
    updated.push_str(&format!("{host} {port} {fingerprint}\n"));
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let filename = path
        .file_name()
        .ok_or_else(SshError::failed)?
        .to_string_lossy();
    let temporary = parent.join(format!(".{filename}.{}.tmp", uuid::Uuid::new_v4()));
    let write_result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| SshError::failed())?;
        file.write_all(updated.as_bytes())
            .map_err(|_| SshError::failed())?;
        file.sync_all().map_err(|_| SshError::failed())?;
        replace_atomically(&temporary, path)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result?;
    Ok(fingerprint)
}

#[cfg(not(windows))]
fn replace_atomically(temporary: &Path, destination: &Path) -> Result<(), SshError> {
    fs::rename(temporary, destination).map_err(|_| SshError::failed())
}

#[cfg(windows)]
fn replace_atomically(temporary: &Path, destination: &Path) -> Result<(), SshError> {
    use std::os::windows::ffi::OsStrExt;
    let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // MoveFileExW with REPLACE_EXISTING performs a same-volume atomic rename.
    let result = unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(SshError::failed())
    } else {
        Ok(())
    }
}

async fn authenticate(
    session: &mut Handle<HostVerifier>,
    username: &str,
    auth: SshAuth,
) -> Result<(), SshError> {
    let accepted = match auth {
        SshAuth::Password(mut password) => {
            let result = session
                .authenticate_password(username, password.as_str())
                .await;
            // Reduce how long the copied password remains live in this buffer.
            password.clear();
            result
                .map(|r| r.success())
                .map_err(|_| SshError::failed())?
        }
        SshAuth::PrivateKey { path, passphrase } => {
            let key =
                load_secret_key(path, passphrase.as_deref()).map_err(|_| SshError::failed())?;
            let key = PrivateKeyWithHashAlg::new(Arc::new(key), None);
            session
                .authenticate_publickey(username, key)
                .await
                .map(|r| r.success())
                .map_err(|_| SshError::failed())?
        }
        SshAuth::WindowsAgent { pipe } => {
            #[cfg(windows)]
            {
                let mut agent = AgentClient::connect_named_pipe(pipe.as_os_str())
                    .await
                    .map_err(|_| SshError::AgentUnavailable)?;
                let identities = agent
                    .request_identities()
                    .await
                    .map_err(|_| SshError::AgentUnavailable)?;
                if identities.is_empty() {
                    return Err(SshError::AgentIdentityRejected);
                }
                let mut success = false;
                for identity in identities {
                    let key = identity.public_key().into_owned();
                    let result = session
                        .authenticate_publickey_with(username, key, None, &mut agent)
                        .await
                        .map_err(|_| SshError::AgentUnavailable)?;
                    if result.success() {
                        success = true;
                        break;
                    }
                }
                if success {
                    true
                } else {
                    return Err(SshError::AgentIdentityRejected);
                }
            }
            #[cfg(not(windows))]
            {
                let _ = pipe;
                return Err(SshError::failed());
            }
        }
    };
    if accepted {
        Ok(())
    } else {
        Err(SshError::AuthenticationRejected)
    }
}

/// An established TCP loopback forward. Dropping it requests orderly shutdown.
pub struct SshTunnel {
    local_port: u16,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl SshTunnel {
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    pub async fn close(mut self) {
        self.request_shutdown();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }

    fn request_shutdown(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        self.request_shutdown();
    }
}

/// Establish an SSH session and loopback TCP forwarding to a database host.
/// When SQL TLS is enabled, the SQL client must connect through the returned
/// loopback port while verifying the original database hostname. This module
/// never changes SQL TLS policy or provides a downgrade path.
pub async fn open_tunnel(
    endpoint: SshEndpoint,
    auth: SshAuth,
    _database_tls: DatabaseTls,
) -> Result<SshTunnel, SshError> {
    if endpoint.host.is_empty()
        || endpoint.username.is_empty()
        || endpoint.port == 0
        || endpoint.database_port == 0
        || endpoint.database_host.is_empty()
    {
        return Err(SshError::failed());
    }

    let (known_fingerprints, host_is_known) =
        load_known_hosts(&endpoint.known_hosts_path, &endpoint.host, endpoint.port)?;
    let key_state = Arc::new(std::sync::Mutex::new(HostKeyState { rejection: None }));
    let config = Arc::new(client::Config::default());
    let handler = HostVerifier {
        known_fingerprints,
        host_is_known,
        state: Arc::clone(&key_state),
    };
    let mut session = client::connect(config, (endpoint.host.as_str(), endpoint.port), handler)
        .await
        .map_err(|_| {
            key_state
                .lock()
                .ok()
                .and_then(|mut state| state.rejection.take())
                .unwrap_or_else(SshError::failed)
        })?;
    authenticate(&mut session, &endpoint.username, auth).await?;

    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .map_err(|_| SshError::failed())?;
    let local_port = listener
        .local_addr()
        .map_err(|_| SshError::failed())?
        .port();
    let session = Arc::new(Mutex::new(session));
    let target_host = endpoint.database_host;
    let target_port = endpoint.database_port;
    let (shutdown, mut shutdown_rx) = oneshot::channel();

    let task = tokio::spawn(async move {
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                _ = connections.join_next(), if !connections.is_empty() => {},
                accepted = listener.accept() => {
                    let Ok((mut local, _peer)) = accepted else { break };
                    let session = Arc::clone(&session);
                    let target_host = target_host.clone();
                    connections.spawn(async move {
                        let Ok(channel) = session.lock().await.channel_open_direct_tcpip(
                            target_host,
                            u32::from(target_port),
                            "127.0.0.1",
                            0,
                        ).await else { return };
                        let mut remote = channel.into_stream();
                        let _ = copy_bidirectional(&mut local, &mut remote).await;
                    });
                }
            }
        }

        connections.abort_all();
        while connections.join_next().await.is_some() {}
        if let Ok(session) = Arc::try_unwrap(session) {
            let session = session.into_inner();
            let _ = session
                .disconnect(Disconnect::ByApplication, "tunnel closed", "en")
                .await;
        }
    });

    Ok(SshTunnel {
        local_port,
        shutdown: Some(shutdown),
        task: Some(task),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_hosts_requires_exact_host_port_fingerprint_line() {
        let path = std::env::temp_dir().join(format!("dbsual-known-hosts-{}", std::process::id()));
        fs::write(
            &path,
            "# explicit entry\ndb.example 22 SHA256:abc\nother.example 22 SHA256:def\n",
        )
        .unwrap();
        let (fingerprints, known) = load_known_hosts(&path, "db.example", 22).unwrap();
        let _ = fs::remove_file(path);
        assert!(known);
        assert_eq!(fingerprints, ["SHA256:abc"]);
    }
}
