//! Key material for the local artifact vault.
//!
//! This module only creates and unlocks the vault key. It does not enable
//! database writes or claim that any database backup is recoverable.

use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use argon2::{Algorithm, Argon2, Params, Version};
use bip39::{Language, Mnemonic};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

const ENVELOPE_VERSION: u8 = 1;
const KEY_BYTES: usize = 32;
const SALT_BYTES: usize = 16;
const NONCE_BYTES: usize = 12;
const WRAPPED_KEY_BYTES: usize = KEY_BYTES + 16;
const ARTIFACT_MAGIC: &[u8; 8] = b"DBSUALA1";
const ARTIFACT_CHUNK_BYTES: usize = 64 * 1024;
const ARTIFACT_MAX_CHUNKS: u64 = 1 << 20;
const ARTIFACT_HEADER_BYTES: usize = 8 + 16 + 16 + 4 + 4 + 2 + 12 + 16 + 12 + 48 + 12 + 48;
const ARGON_MEMORY_KIB: u32 = 65_536;
const ARGON_ITERATIONS: u32 = 3;
const ARGON_LANES: u32 = 4;
const ARGON_ALGORITHM: &str = "argon2id-v1.3";
const ARGON_ALGORITHM_ID: u8 = 1;
const KEYRING_SERVICE: &str = "com.dbsual.desktop.artifact-vault";
static KEYRING_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryEnvelope {
    version: u8,
    vault_id: Uuid,
    argon_algorithm: String,
    argon_memory_kib: u32,
    argon_iterations: u32,
    argon_lanes: u32,
    argon_output_bytes: u8,
    salt: Vec<u8>,
    nonce: Vec<u8>,
    wrapped_master_key: Vec<u8>,
}

impl RecoveryEnvelope {
    pub fn vault_id(&self) -> Uuid {
        self.vault_id
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KeyFileEnvelope {
    version: u8,
    vault_id: Uuid,
    argon_algorithm: String,
    argon_memory_kib: u32,
    argon_iterations: u32,
    argon_lanes: u32,
    argon_output_bytes: u8,
    salt: Vec<u8>,
    nonce: Vec<u8>,
    wrapped_master_key: Vec<u8>,
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct MasterKey([u8; KEY_BYTES]);

impl MasterKey {
    pub fn expose_for_keyring(&self) -> &[u8; KEY_BYTES] {
        &self.0
    }
}

pub struct NewVault {
    pub vault_id: Uuid,
    pub recovery_phrase: String,
    pub master_key: MasterKey,
    pub recovery_envelope: RecoveryEnvelope,
}

impl NewVault {
    /// Copy the key material needed by a background artifact writer. The copy
    /// remains zeroized on drop just like the original key.
    #[cfg(test)]
    pub(crate) fn artifact_material(&self) -> (MasterKey, RecoveryEnvelope) {
        (MasterKey(self.master_key.0), self.recovery_envelope.clone())
    }
}

impl Drop for NewVault {
    fn drop(&mut self) {
        self.recovery_phrase.zeroize();
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum VaultError {
    RandomnessUnavailable,
    InvalidRecoveryPhrase,
    UnsupportedEnvelope,
    InvalidEnvelope,
    DerivationFailed,
    AuthenticationFailed,
    KeyringUnavailable,
    IoFailure,
    InvalidArtifact,
    ArtifactTooLarge,
    WeakKeyFilePassword,
}

pub fn create_vault() -> Result<NewVault, VaultError> {
    let mut master = Zeroizing::new([0_u8; KEY_BYTES]);
    random_bytes(&mut master[..])?;
    let vault_id = Uuid::new_v4();
    let master_key = MasterKey(*master);
    prepare_recovered_vault(vault_id, master_key)
}

/// Prepare a new local recovery phrase for a key imported from a portable file.
/// The vault ID and master key remain unchanged so existing artifacts stay valid.
pub fn prepare_recovered_vault(
    vault_id: Uuid,
    master_key: MasterKey,
) -> Result<NewVault, VaultError> {
    let mut phrase_entropy = Zeroizing::new([0_u8; 16]);
    random_bytes(&mut phrase_entropy[..])?;
    let mnemonic = Mnemonic::from_entropy_in(Language::Spanish, &phrase_entropy[..])
        .map_err(|_| VaultError::InvalidRecoveryPhrase)?;
    phrase_entropy.zeroize();
    let recovery_phrase = mnemonic.to_string();
    let recovery_envelope = wrap_master_key(vault_id, &master_key, &recovery_phrase)?;

    Ok(NewVault {
        vault_id,
        recovery_phrase,
        master_key,
        recovery_envelope,
    })
}

pub fn recover_master_key(
    envelope: &RecoveryEnvelope,
    phrase: &str,
) -> Result<MasterKey, VaultError> {
    if envelope.version != ENVELOPE_VERSION {
        return Err(VaultError::UnsupportedEnvelope);
    }
    if !matches_supported_kdf(
        &envelope.argon_algorithm,
        envelope.argon_memory_kib,
        envelope.argon_iterations,
        envelope.argon_lanes,
        envelope.argon_output_bytes,
    ) {
        return Err(VaultError::UnsupportedEnvelope);
    }
    if envelope.salt.len() != SALT_BYTES
        || envelope.nonce.len() != NONCE_BYTES
        || envelope.wrapped_master_key.len() != WRAPPED_KEY_BYTES
    {
        return Err(VaultError::InvalidEnvelope);
    }

    let normalized_phrase = normalized_mnemonic(phrase)?;
    let salt: [u8; SALT_BYTES] = envelope
        .salt
        .as_slice()
        .try_into()
        .map_err(|_| VaultError::InvalidEnvelope)?;
    let nonce: [u8; NONCE_BYTES] = envelope
        .nonce
        .as_slice()
        .try_into()
        .map_err(|_| VaultError::InvalidEnvelope)?;
    let mut wrapping_key = derive_wrapping_key(normalized_phrase.as_bytes(), &salt)?;
    let cipher =
        Aes256Gcm::new_from_slice(&wrapping_key.0).map_err(|_| VaultError::DerivationFailed)?;
    let nonce = Nonce::from(nonce);
    let mut plaintext = cipher
        .decrypt(
            &nonce,
            Payload {
                msg: &envelope.wrapped_master_key,
                aad: &recovery_aad(envelope.vault_id),
            },
        )
        .map_err(|_| VaultError::AuthenticationFailed)?;
    wrapping_key.zeroize();

    let mut master = [0_u8; KEY_BYTES];
    if plaintext.len() != KEY_BYTES {
        plaintext.zeroize();
        return Err(VaultError::InvalidEnvelope);
    }
    master.copy_from_slice(&plaintext);
    plaintext.zeroize();
    let result = MasterKey(master);
    master.zeroize();
    Ok(result)
}

/// Create a portable JSON key file. The key file contains only the vault ID,
/// KDF salt, nonce, and authenticated ciphertext; it never contains the
/// password or an unwrapped master key.
pub fn export_key_file(
    vault_id: Uuid,
    master_key: &MasterKey,
    password: &str,
) -> Result<Vec<u8>, VaultError> {
    validate_key_file_password(password)?;
    let mut salt = [0_u8; SALT_BYTES];
    let mut nonce = [0_u8; NONCE_BYTES];
    random_bytes(&mut salt)?;
    random_bytes(&mut nonce)?;
    let mut wrapping_key = derive_wrapping_key(password.as_bytes(), &salt)?;
    let cipher =
        Aes256Gcm::new_from_slice(&wrapping_key.0).map_err(|_| VaultError::DerivationFailed)?;
    let nonce_value = Nonce::from(nonce);
    let wrapped_master_key = cipher
        .encrypt(
            &nonce_value,
            Payload {
                msg: &master_key.0,
                aad: &key_file_aad(vault_id),
            },
        )
        .map_err(|_| VaultError::AuthenticationFailed)?;
    wrapping_key.zeroize();

    serde_json::to_vec(&KeyFileEnvelope {
        version: ENVELOPE_VERSION,
        vault_id,
        argon_algorithm: ARGON_ALGORITHM.to_owned(),
        argon_memory_kib: ARGON_MEMORY_KIB,
        argon_iterations: ARGON_ITERATIONS,
        argon_lanes: ARGON_LANES,
        argon_output_bytes: KEY_BYTES as u8,
        salt: salt.to_vec(),
        nonce: nonce.to_vec(),
        wrapped_master_key,
    })
    .map_err(|_| VaultError::InvalidEnvelope)
}

/// Write a portable key file to a user-selected path without replacing an
/// existing file. Publication is atomic on the same filesystem.
pub fn export_key_file_to_path(
    destination: &Path,
    vault_id: Uuid,
    master_key: &MasterKey,
    password: &str,
) -> Result<(), VaultError> {
    let encrypted = Zeroizing::new(export_key_file(vault_id, master_key, password)?);
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let file_name = destination
        .file_name()
        .ok_or(VaultError::IoFailure)?
        .to_string_lossy();
    let temporary_path = parent.join(format!(".{file_name}.{}.partial", Uuid::new_v4()));
    let _temporary = TemporaryArtifact(temporary_path.clone());
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary_path)
        .map_err(|_| VaultError::IoFailure)?;
    let mut writer = BufWriter::new(file);
    writer
        .write_all(&encrypted)
        .map_err(|_| VaultError::IoFailure)?;
    writer.flush().map_err(|_| VaultError::IoFailure)?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|_| VaultError::IoFailure)?;
    drop(writer);
    publish_without_overwrite(&temporary_path, destination)
}

/// Import a key file after bounding and validating its public envelope.
pub fn import_key_file(bytes: &[u8], password: &str) -> Result<(Uuid, MasterKey), VaultError> {
    const MAX_KEY_FILE_BYTES: usize = 4096;
    if bytes.is_empty() || bytes.len() > MAX_KEY_FILE_BYTES {
        return Err(VaultError::InvalidEnvelope);
    }
    validate_key_file_password(password)?;
    let envelope: KeyFileEnvelope =
        serde_json::from_slice(bytes).map_err(|_| VaultError::InvalidEnvelope)?;
    if envelope.version != ENVELOPE_VERSION {
        return Err(VaultError::UnsupportedEnvelope);
    }
    if !matches_supported_kdf(
        &envelope.argon_algorithm,
        envelope.argon_memory_kib,
        envelope.argon_iterations,
        envelope.argon_lanes,
        envelope.argon_output_bytes,
    ) {
        return Err(VaultError::UnsupportedEnvelope);
    }
    if envelope.salt.len() != SALT_BYTES
        || envelope.nonce.len() != NONCE_BYTES
        || envelope.wrapped_master_key.len() != WRAPPED_KEY_BYTES
    {
        return Err(VaultError::InvalidEnvelope);
    }

    let salt: [u8; SALT_BYTES] = envelope
        .salt
        .as_slice()
        .try_into()
        .map_err(|_| VaultError::InvalidEnvelope)?;
    let nonce: [u8; NONCE_BYTES] = envelope
        .nonce
        .as_slice()
        .try_into()
        .map_err(|_| VaultError::InvalidEnvelope)?;
    let mut wrapping_key = derive_wrapping_key(password.as_bytes(), &salt)?;
    let cipher =
        Aes256Gcm::new_from_slice(&wrapping_key.0).map_err(|_| VaultError::DerivationFailed)?;
    let mut plaintext = cipher
        .decrypt(
            &Nonce::from(nonce),
            Payload {
                msg: &envelope.wrapped_master_key,
                aad: &key_file_aad(envelope.vault_id),
            },
        )
        .map_err(|_| VaultError::AuthenticationFailed)?;
    wrapping_key.zeroize();
    if plaintext.len() != KEY_BYTES {
        plaintext.zeroize();
        return Err(VaultError::InvalidEnvelope);
    }
    let mut key = [0_u8; KEY_BYTES];
    key.copy_from_slice(&plaintext);
    plaintext.zeroize();
    let master_key = MasterKey(key);
    key.zeroize();
    Ok((envelope.vault_id, master_key))
}

fn validate_key_file_password(password: &str) -> Result<(), VaultError> {
    if password.trim().chars().count() < 12 || password.len() > 1024 {
        return Err(VaultError::WeakKeyFilePassword);
    }
    Ok(())
}

fn matches_supported_kdf(
    algorithm: &str,
    memory_kib: u32,
    iterations: u32,
    lanes: u32,
    output_bytes: u8,
) -> bool {
    algorithm == ARGON_ALGORITHM
        && memory_kib == ARGON_MEMORY_KIB
        && iterations == ARGON_ITERATIONS
        && lanes == ARGON_LANES
        && output_bytes == KEY_BYTES as u8
}

/// Store the vault master key in the current Windows user's credential store.
/// Call this function from a blocking worker because the platform keyring API
/// is synchronous.
pub fn store_master_key(vault_id: Uuid, master_key: &MasterKey) -> Result<(), VaultError> {
    let _guard = KEYRING_LOCK
        .lock()
        .map_err(|_| VaultError::KeyringUnavailable)?;
    let entry = keyring::Entry::new(KEYRING_SERVICE, &vault_id.to_string())
        .map_err(|_| VaultError::KeyringUnavailable)?;
    entry
        .set_secret(master_key.expose_for_keyring())
        .map_err(|_| VaultError::KeyringUnavailable)
}

/// Load the master key from the current Windows user's credential store.
/// Call this function from a blocking worker because the platform keyring API
/// is synchronous.
pub fn load_master_key(vault_id: Uuid) -> Result<MasterKey, VaultError> {
    let _guard = KEYRING_LOCK
        .lock()
        .map_err(|_| VaultError::KeyringUnavailable)?;
    let entry = keyring::Entry::new(KEYRING_SERVICE, &vault_id.to_string())
        .map_err(|_| VaultError::KeyringUnavailable)?;
    let mut secret = entry
        .get_secret()
        .map_err(|_| VaultError::KeyringUnavailable)?;
    if secret.len() != KEY_BYTES {
        secret.zeroize();
        return Err(VaultError::KeyringUnavailable);
    }
    let mut key = [0_u8; KEY_BYTES];
    key.copy_from_slice(&secret);
    secret.zeroize();
    let result = MasterKey(key);
    key.zeroize();
    Ok(result)
}

/// Remove a vault key only as part of an explicit vault deletion flow.
pub fn remove_master_key(vault_id: Uuid) -> Result<(), VaultError> {
    let _guard = KEYRING_LOCK
        .lock()
        .map_err(|_| VaultError::KeyringUnavailable)?;
    let entry = keyring::Entry::new(KEYRING_SERVICE, &vault_id.to_string())
        .map_err(|_| VaultError::KeyringUnavailable)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(VaultError::KeyringUnavailable),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArtifactInfo {
    pub vault_id: Uuid,
    pub artifact_id: Uuid,
    pub plaintext_bytes: u64,
    pub encrypted_chunks: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedArtifactInfo {
    pub artifact: ArtifactInfo,
    pub encrypted_bytes: u64,
    pub ciphertext_sha256: String,
}

struct TemporaryArtifact(PathBuf);

impl Drop for TemporaryArtifact {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

struct ArtifactHeader {
    vault_id: Uuid,
    artifact_id: Uuid,
    recovery: RecoveryEnvelope,
    nonce_prefix: [u8; 4],
    data_key_nonce: [u8; NONCE_BYTES],
    wrapped_data_key: [u8; WRAPPED_KEY_BYTES],
    bytes: [u8; ARTIFACT_HEADER_BYTES],
}

/// Encrypt an input stream into the portable DBSUAL artifact format.
/// The caller must write to a new temporary file and publish it only after
/// this function returns successfully and the file has been flushed.
pub fn encrypt_artifact<R: Read, W: Write>(
    input: &mut R,
    output: &mut W,
    vault_id: Uuid,
    master_key: &MasterKey,
    recovery_phrase: &str,
) -> Result<ArtifactInfo, VaultError> {
    let recovery = wrap_master_key(vault_id, master_key, recovery_phrase)?;
    encrypt_artifact_with_recovery(input, output, vault_id, master_key, &recovery)
}

/// Encrypt with a previously verified recovery envelope already stored for
/// this vault. This lets the app create artifacts without retaining the phrase.
pub fn encrypt_artifact_with_recovery<R: Read, W: Write>(
    input: &mut R,
    output: &mut W,
    vault_id: Uuid,
    master_key: &MasterKey,
    recovery: &RecoveryEnvelope,
) -> Result<ArtifactInfo, VaultError> {
    if recovery.vault_id != vault_id
        || recovery.version != ENVELOPE_VERSION
        || !matches_supported_kdf(
            &recovery.argon_algorithm,
            recovery.argon_memory_kib,
            recovery.argon_iterations,
            recovery.argon_lanes,
            recovery.argon_output_bytes,
        )
        || recovery.salt.len() != SALT_BYTES
        || recovery.nonce.len() != NONCE_BYTES
        || recovery.wrapped_master_key.len() != WRAPPED_KEY_BYTES
    {
        return Err(VaultError::InvalidEnvelope);
    }
    let artifact_id = Uuid::new_v4();
    let mut data_key_bytes = Zeroizing::new([0_u8; KEY_BYTES]);
    random_bytes(&mut data_key_bytes[..])?;
    let mut data_key = MasterKey(*data_key_bytes);
    let mut data_key_nonce = [0_u8; NONCE_BYTES];
    let mut nonce_prefix = [0_u8; 4];
    random_bytes(&mut data_key_nonce)?;
    random_bytes(&mut nonce_prefix)?;
    let data_cipher =
        Aes256Gcm::new_from_slice(&master_key.0).map_err(|_| VaultError::DerivationFailed)?;
    let wrapped_data_key = data_cipher
        .encrypt(
            &Nonce::from(data_key_nonce),
            Payload {
                msg: &data_key.0,
                aad: &data_key_aad(vault_id, artifact_id),
            },
        )
        .map_err(|_| VaultError::AuthenticationFailed)?;
    data_key.zeroize();
    let wrapped_data_key: [u8; WRAPPED_KEY_BYTES] = wrapped_data_key
        .try_into()
        .map_err(|_| VaultError::InvalidArtifact)?;
    let header = build_artifact_header(
        vault_id,
        artifact_id,
        recovery.clone(),
        nonce_prefix,
        data_key_nonce,
        wrapped_data_key,
    )?;
    output
        .write_all(&header.bytes)
        .map_err(|_| VaultError::IoFailure)?;
    let header_hash: [u8; 32] = Sha256::digest(header.bytes).into();
    let cipher =
        Aes256Gcm::new_from_slice(&data_key_bytes[..]).map_err(|_| VaultError::DerivationFailed)?;

    let mut total_bytes = 0_u64;
    let mut chunks = 0_u64;
    let mut plain = Zeroizing::new(vec![0_u8; ARTIFACT_CHUNK_BYTES]);
    loop {
        let mut used = 0;
        while used < ARTIFACT_CHUNK_BYTES {
            let count = input
                .read(&mut plain[used..])
                .map_err(|_| VaultError::IoFailure)?;
            if count == 0 {
                break;
            }
            used += count;
        }
        if used == 0 {
            break;
        }
        if chunks >= ARTIFACT_MAX_CHUNKS {
            data_key_bytes.zeroize();
            return Err(VaultError::ArtifactTooLarge);
        }
        let length = used as u32;
        let nonce_bytes = record_nonce(nonce_prefix, chunks);
        let aad = chunk_aad(&header_hash, artifact_id, chunks, length);
        let encrypted = cipher
            .encrypt(
                &Nonce::from(nonce_bytes),
                Payload {
                    msg: &plain[..used],
                    aad: &aad,
                },
            )
            .map_err(|_| VaultError::AuthenticationFailed)?;
        output
            .write_all(&chunks.to_le_bytes())
            .and_then(|_| output.write_all(&length.to_le_bytes()))
            .and_then(|_| output.write_all(&nonce_bytes))
            .and_then(|_| output.write_all(&encrypted))
            .map_err(|_| VaultError::IoFailure)?;
        plain[..used].zeroize();
        total_bytes = total_bytes
            .checked_add(u64::from(length))
            .ok_or(VaultError::ArtifactTooLarge)?;
        chunks += 1;
        if used < ARTIFACT_CHUNK_BYTES {
            break;
        }
    }

    let footer_nonce = record_nonce(nonce_prefix, u64::MAX);
    let mut footer = Zeroizing::new(Vec::with_capacity(16));
    footer.extend_from_slice(&total_bytes.to_le_bytes());
    footer.extend_from_slice(&chunks.to_le_bytes());
    let footer = cipher
        .encrypt(
            &Nonce::from(footer_nonce),
            Payload {
                msg: &footer,
                aad: &footer_aad(&header_hash, artifact_id),
            },
        )
        .map_err(|_| VaultError::AuthenticationFailed)?;
    output
        .write_all(&u64::MAX.to_le_bytes())
        .and_then(|_| output.write_all(&0_u32.to_le_bytes()))
        .and_then(|_| output.write_all(&footer_nonce))
        .and_then(|_| output.write_all(&footer))
        .map_err(|_| VaultError::IoFailure)?;
    data_key_bytes.zeroize();

    Ok(ArtifactInfo {
        vault_id,
        artifact_id,
        plaintext_bytes: total_bytes,
        encrypted_chunks: chunks,
    })
}

/// Encrypt small sensitive content directly into an authenticated artifact
/// file without writing its plaintext to disk.
pub fn encrypt_artifact_bytes_to_file(
    directory: &Path,
    plaintext: &[u8],
    vault_id: Uuid,
    master_key: &MasterKey,
    recovery: &RecoveryEnvelope,
) -> Result<ArtifactInfo, VaultError> {
    fs::create_dir_all(directory).map_err(|_| VaultError::IoFailure)?;
    let mut reader = std::io::Cursor::new(plaintext);
    let mut encrypted = Vec::new();
    let info = encrypt_artifact_with_recovery(
        &mut reader,
        &mut encrypted,
        vault_id,
        master_key,
        recovery,
    )?;
    let file_name = format!("{}.dbsual-artifact", info.artifact_id);
    let destination = directory.join(file_name);
    let temporary = directory.join(format!(".{}.partial", Uuid::new_v4()));
    let _temporary = TemporaryArtifact(temporary.clone());
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|_| VaultError::IoFailure)?;
    output
        .write_all(&encrypted)
        .and_then(|_| output.sync_all())
        .map_err(|_| VaultError::IoFailure)?;
    drop(output);

    let file = File::open(&temporary).map_err(|_| VaultError::IoFailure)?;
    let mut verify_reader = BufReader::new(file);
    let verify_header = read_artifact_header(&mut verify_reader)?;
    let verified = decrypt_artifact_with_master(
        &mut verify_reader,
        &mut std::io::sink(),
        verify_header,
        master_key,
    )?;
    if verified != info {
        return Err(VaultError::InvalidArtifact);
    }
    publish_without_overwrite(&temporary, &destination)?;
    Ok(info)
}

/// Encrypt a file into a sibling temporary file and publish it only after
/// verification. Same-volume hard-link creation atomically refuses replacement.
pub fn encrypt_artifact_file(
    source: &Path,
    destination: &Path,
    vault_id: Uuid,
    master_key: &MasterKey,
    recovery_phrase: &str,
) -> Result<ArtifactInfo, VaultError> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let file_name = destination
        .file_name()
        .ok_or(VaultError::IoFailure)?
        .to_string_lossy();
    let temporary_path = parent.join(format!(".{file_name}.{}.partial", Uuid::new_v4()));
    let _temporary = TemporaryArtifact(temporary_path.clone());
    let source_file = File::open(source).map_err(|_| VaultError::IoFailure)?;
    let target_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary_path)
        .map_err(|_| VaultError::IoFailure)?;
    let mut reader = BufReader::new(source_file);
    let mut writer = BufWriter::new(target_file);
    let info = encrypt_artifact(
        &mut reader,
        &mut writer,
        vault_id,
        master_key,
        recovery_phrase,
    )?;
    writer.flush().map_err(|_| VaultError::IoFailure)?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|_| VaultError::IoFailure)?;
    drop(writer);
    let verify_file = File::open(&temporary_path).map_err(|_| VaultError::IoFailure)?;
    let mut verify_reader = BufReader::new(verify_file);
    let verify_header = read_artifact_header(&mut verify_reader)?;
    let verified = decrypt_artifact_with_master(
        &mut verify_reader,
        &mut std::io::sink(),
        verify_header,
        master_key,
    )?;
    drop(verify_reader);
    if verified != info {
        return Err(VaultError::InvalidArtifact);
    }
    publish_without_overwrite(&temporary_path, destination)?;
    Ok(info)
}

/// Encrypt an arbitrary reader directly into a published artifact. This is
/// the streaming boundary for database backup providers: a dump can flow from
/// its producer to authenticated ciphertext without a plaintext staging file.
pub fn encrypt_artifact_reader_to_directory<R: Read>(
    directory: &Path,
    input: &mut R,
    vault_id: Uuid,
    master_key: &MasterKey,
    recovery: &RecoveryEnvelope,
) -> Result<PublishedArtifactInfo, VaultError> {
    fs::create_dir_all(directory).map_err(|_| VaultError::IoFailure)?;
    let temporary_path = directory.join(format!(".{}.partial", Uuid::new_v4()));
    let _temporary = TemporaryArtifact(temporary_path.clone());
    let output_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary_path)
        .map_err(|_| VaultError::IoFailure)?;
    let mut output = BufWriter::new(output_file);
    let info = encrypt_artifact_with_recovery(input, &mut output, vault_id, master_key, recovery)?;
    output.flush().map_err(|_| VaultError::IoFailure)?;
    output
        .get_ref()
        .sync_all()
        .map_err(|_| VaultError::IoFailure)?;
    drop(output);

    let verify_file = File::open(&temporary_path).map_err(|_| VaultError::IoFailure)?;
    let mut verify_reader = BufReader::new(verify_file);
    let verify_header = read_artifact_header(&mut verify_reader)?;
    let verified = decrypt_artifact_with_master(
        &mut verify_reader,
        &mut std::io::sink(),
        verify_header,
        master_key,
    )?;
    drop(verify_reader);
    if verified != info {
        return Err(VaultError::InvalidArtifact);
    }
    let destination = directory.join(format!("{}.dbsual-artifact", info.artifact_id));
    let encrypted_bytes = fs::metadata(&temporary_path)
        .map_err(|_| VaultError::IoFailure)?
        .len();
    let mut hash_reader =
        BufReader::new(File::open(&temporary_path).map_err(|_| VaultError::IoFailure)?);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = hash_reader
            .read(&mut buffer)
            .map_err(|_| VaultError::IoFailure)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let ciphertext_sha256 = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    publish_without_overwrite(&temporary_path, &destination)?;
    Ok(PublishedArtifactInfo {
        artifact: info,
        encrypted_bytes,
        ciphertext_sha256,
    })
}

/// Authenticate and decrypt an artifact stream. The output receives only
/// individually authenticated chunks; callers must direct it to a controlled
/// staging destination and commit/rename only after this function succeeds.
pub fn decrypt_artifact<R: Read, W: Write>(
    input: &mut R,
    output: &mut W,
    recovery_phrase: &str,
) -> Result<ArtifactInfo, VaultError> {
    let header = read_artifact_header(input)?;
    let master_key = recover_master_key(&header.recovery, recovery_phrase)?;
    decrypt_artifact_with_master(input, output, header, &master_key)
}

/// Decrypt a portable artifact with an imported key file. Keep `output`
/// provisional until this returns successfully, because the authenticated
/// stream footer is checked after the final plaintext block is written.
pub fn decrypt_artifact_with_key_file<R: Read, W: Write>(
    input: &mut R,
    output: &mut W,
    key_file: &[u8],
    password: &str,
) -> Result<ArtifactInfo, VaultError> {
    let header = read_artifact_header(input)?;
    let (vault_id, master_key) = import_key_file(key_file, password)?;
    if vault_id != header.vault_id {
        return Err(VaultError::AuthenticationFailed);
    }
    decrypt_artifact_with_master(input, output, header, &master_key)
}

/// Decrypt to a sibling staging file and publish only after authenticating EOF.
pub fn decrypt_artifact_file(
    source: &Path,
    destination: &Path,
    recovery_phrase: &str,
) -> Result<ArtifactInfo, VaultError> {
    decrypt_artifact_file_with(source, destination, |input, output| {
        decrypt_artifact(input, output, recovery_phrase)
    })
}

/// Decrypt to a staged file using a portable key file and its password.
pub fn decrypt_artifact_file_with_key_file(
    source: &Path,
    destination: &Path,
    key_file: &[u8],
    password: &str,
) -> Result<ArtifactInfo, VaultError> {
    decrypt_artifact_file_with(source, destination, |input, output| {
        decrypt_artifact_with_key_file(input, output, key_file, password)
    })
}

fn decrypt_artifact_file_with<F>(
    source: &Path,
    destination: &Path,
    decrypt: F,
) -> Result<ArtifactInfo, VaultError>
where
    F: FnOnce(&mut BufReader<File>, &mut BufWriter<File>) -> Result<ArtifactInfo, VaultError>,
{
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let file_name = destination
        .file_name()
        .ok_or(VaultError::IoFailure)?
        .to_string_lossy();
    let temporary_path = parent.join(format!(".{file_name}.{}.partial", Uuid::new_v4()));
    let _temporary = TemporaryArtifact(temporary_path.clone());
    let input_file = File::open(source).map_err(|_| VaultError::IoFailure)?;
    let output_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary_path)
        .map_err(|_| VaultError::IoFailure)?;
    let mut input = BufReader::new(input_file);
    let mut output = BufWriter::new(output_file);
    let info = decrypt(&mut input, &mut output)?;
    output.flush().map_err(|_| VaultError::IoFailure)?;
    output
        .get_ref()
        .sync_all()
        .map_err(|_| VaultError::IoFailure)?;
    drop(output);
    publish_without_overwrite(&temporary_path, destination)?;
    Ok(info)
}

#[cfg(windows)]
pub(crate) fn publish_without_overwrite(
    temporary: &Path,
    destination: &Path,
) -> Result<(), VaultError> {
    use std::os::windows::ffi::OsStrExt;
    let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // Without MOVEFILE_REPLACE_EXISTING the rename is atomic and fails if the
    // destination exists. WRITE_THROUGH asks Windows to flush the move.
    let moved = unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        return Err(VaultError::IoFailure);
    }
    Ok(())
}

#[cfg(not(windows))]
pub(crate) fn publish_without_overwrite(
    temporary: &Path,
    destination: &Path,
) -> Result<(), VaultError> {
    fs::hard_link(temporary, destination).map_err(|_| VaultError::IoFailure)?;
    let _ = fs::remove_file(temporary);
    Ok(())
}

/// Decrypt using the vault key in Windows Credential Manager. Call from a
/// blocking worker, and keep the output provisional until success.
pub fn decrypt_artifact_from_keyring<R: Read, W: Write>(
    input: &mut R,
    output: &mut W,
) -> Result<ArtifactInfo, VaultError> {
    let header = read_artifact_header(input)?;
    let master_key = load_master_key(header.vault_id)?;
    decrypt_artifact_with_master(input, output, header, &master_key)
}

/// Authenticate every artifact block and its footer without retaining the
/// plaintext. Call this before any operation that could mutate a destination.
pub fn verify_artifact_from_keyring<R: Read>(input: &mut R) -> Result<ArtifactInfo, VaultError> {
    decrypt_artifact_from_keyring(input, &mut std::io::sink())
}

/// Authenticate and decrypt with a master key already unlocked from Windows
/// Credential Manager. As above, output remains provisional until success.
fn decrypt_artifact_with_master<R: Read, W: Write>(
    input: &mut R,
    output: &mut W,
    header: ArtifactHeader,
    master_key: &MasterKey,
) -> Result<ArtifactInfo, VaultError> {
    let header_hash: [u8; 32] = Sha256::digest(header.bytes).into();
    let wrap_cipher =
        Aes256Gcm::new_from_slice(&master_key.0).map_err(|_| VaultError::DerivationFailed)?;
    let mut data_key_bytes = Zeroizing::new(
        wrap_cipher
            .decrypt(
                &Nonce::from(header.data_key_nonce),
                Payload {
                    msg: &header.wrapped_data_key,
                    aad: &data_key_aad(header.vault_id, header.artifact_id),
                },
            )
            .map_err(|_| VaultError::AuthenticationFailed)?,
    );
    if data_key_bytes.len() != KEY_BYTES {
        data_key_bytes.zeroize();
        return Err(VaultError::InvalidArtifact);
    }
    let cipher =
        Aes256Gcm::new_from_slice(&data_key_bytes[..]).map_err(|_| VaultError::DerivationFailed)?;
    let mut total_bytes = 0_u64;
    let mut expected_index = 0_u64;
    loop {
        let mut index_bytes = [0_u8; 8];
        input
            .read_exact(&mut index_bytes)
            .map_err(|_| VaultError::InvalidArtifact)?;
        let index = u64::from_le_bytes(index_bytes);
        let mut length_bytes = [0_u8; 4];
        input
            .read_exact(&mut length_bytes)
            .map_err(|_| VaultError::InvalidArtifact)?;
        let length = u32::from_le_bytes(length_bytes);
        if index == u64::MAX {
            if length != 0 || expected_index > ARTIFACT_MAX_CHUNKS {
                data_key_bytes.zeroize();
                return Err(VaultError::InvalidArtifact);
            }
            let mut nonce_bytes = [0_u8; NONCE_BYTES];
            input
                .read_exact(&mut nonce_bytes)
                .map_err(|_| VaultError::InvalidArtifact)?;
            if nonce_bytes != record_nonce(header.nonce_prefix, u64::MAX) {
                return Err(VaultError::InvalidArtifact);
            }
            let mut encrypted_footer = [0_u8; 32];
            input
                .read_exact(&mut encrypted_footer)
                .map_err(|_| VaultError::InvalidArtifact)?;
            let mut footer = Zeroizing::new(
                cipher
                    .decrypt(
                        &Nonce::from(nonce_bytes),
                        Payload {
                            msg: &encrypted_footer,
                            aad: &footer_aad(&header_hash, header.artifact_id),
                        },
                    )
                    .map_err(|_| VaultError::AuthenticationFailed)?,
            );
            if footer.len() != 16
                || u64::from_le_bytes(footer[..8].try_into().unwrap()) != total_bytes
                || u64::from_le_bytes(footer[8..].try_into().unwrap()) != expected_index
            {
                footer.zeroize();
                data_key_bytes.zeroize();
                return Err(VaultError::InvalidArtifact);
            }
            footer.zeroize();
            let mut extra = [0_u8; 1];
            if input.read(&mut extra).map_err(|_| VaultError::IoFailure)? != 0 {
                data_key_bytes.zeroize();
                return Err(VaultError::InvalidArtifact);
            }
            data_key_bytes.zeroize();
            return Ok(ArtifactInfo {
                vault_id: header.vault_id,
                artifact_id: header.artifact_id,
                plaintext_bytes: total_bytes,
                encrypted_chunks: expected_index,
            });
        }
        if index != expected_index || length == 0 || length as usize > ARTIFACT_CHUNK_BYTES {
            data_key_bytes.zeroize();
            return Err(VaultError::InvalidArtifact);
        }
        if expected_index >= ARTIFACT_MAX_CHUNKS {
            data_key_bytes.zeroize();
            return Err(VaultError::ArtifactTooLarge);
        }
        let mut nonce_bytes = [0_u8; NONCE_BYTES];
        input
            .read_exact(&mut nonce_bytes)
            .map_err(|_| VaultError::InvalidArtifact)?;
        if nonce_bytes != record_nonce(header.nonce_prefix, expected_index) {
            return Err(VaultError::InvalidArtifact);
        }
        let encrypted_length = length as usize + 16;
        let mut encrypted = vec![0_u8; encrypted_length];
        input
            .read_exact(&mut encrypted)
            .map_err(|_| VaultError::InvalidArtifact)?;
        let aad = chunk_aad(&header_hash, header.artifact_id, expected_index, length);
        let mut plain = Zeroizing::new(
            cipher
                .decrypt(
                    &Nonce::from(nonce_bytes),
                    Payload {
                        msg: &encrypted,
                        aad: &aad,
                    },
                )
                .map_err(|_| VaultError::AuthenticationFailed)?,
        );
        encrypted.zeroize();
        if plain.len() != length as usize {
            data_key_bytes.zeroize();
            return Err(VaultError::InvalidArtifact);
        }
        output
            .write_all(&plain)
            .map_err(|_| VaultError::IoFailure)?;
        plain.zeroize();
        total_bytes = total_bytes
            .checked_add(u64::from(length))
            .ok_or(VaultError::ArtifactTooLarge)?;
        expected_index += 1;
    }
}

fn build_artifact_header(
    vault_id: Uuid,
    artifact_id: Uuid,
    recovery: RecoveryEnvelope,
    nonce_prefix: [u8; 4],
    data_key_nonce: [u8; NONCE_BYTES],
    wrapped_data_key: [u8; WRAPPED_KEY_BYTES],
) -> Result<ArtifactHeader, VaultError> {
    if recovery.version != ENVELOPE_VERSION
        || recovery.vault_id != vault_id
        || !matches_supported_kdf(
            &recovery.argon_algorithm,
            recovery.argon_memory_kib,
            recovery.argon_iterations,
            recovery.argon_lanes,
            recovery.argon_output_bytes,
        )
        || recovery.salt.len() != SALT_BYTES
        || recovery.nonce.len() != NONCE_BYTES
        || recovery.wrapped_master_key.len() != WRAPPED_KEY_BYTES
    {
        return Err(VaultError::InvalidEnvelope);
    }
    let mut bytes = [0_u8; ARTIFACT_HEADER_BYTES];
    let mut offset = 0;
    append(&mut bytes, &mut offset, ARTIFACT_MAGIC);
    append(&mut bytes, &mut offset, vault_id.as_bytes());
    append(&mut bytes, &mut offset, artifact_id.as_bytes());
    append(
        &mut bytes,
        &mut offset,
        &(ARTIFACT_CHUNK_BYTES as u32).to_le_bytes(),
    );
    append(&mut bytes, &mut offset, &nonce_prefix);
    append(&mut bytes, &mut offset, &[ARGON_ALGORITHM_ID]);
    append(&mut bytes, &mut offset, &[KEY_BYTES as u8]);
    append(
        &mut bytes,
        &mut offset,
        &recovery.argon_memory_kib.to_le_bytes(),
    );
    append(
        &mut bytes,
        &mut offset,
        &recovery.argon_iterations.to_le_bytes(),
    );
    append(&mut bytes, &mut offset, &recovery.argon_lanes.to_le_bytes());
    append(&mut bytes, &mut offset, &recovery.salt);
    append(&mut bytes, &mut offset, &recovery.nonce);
    append(&mut bytes, &mut offset, &recovery.wrapped_master_key);
    append(&mut bytes, &mut offset, &data_key_nonce);
    append(&mut bytes, &mut offset, &wrapped_data_key);
    debug_assert_eq!(offset, ARTIFACT_HEADER_BYTES);
    Ok(ArtifactHeader {
        vault_id,
        artifact_id,
        recovery,
        nonce_prefix,
        data_key_nonce,
        wrapped_data_key,
        bytes,
    })
}

fn read_artifact_header<R: Read>(input: &mut R) -> Result<ArtifactHeader, VaultError> {
    let mut bytes = [0_u8; ARTIFACT_HEADER_BYTES];
    input
        .read_exact(&mut bytes)
        .map_err(|_| VaultError::InvalidArtifact)?;
    let mut offset = 0;
    if take::<8>(&bytes, &mut offset)? != *ARTIFACT_MAGIC {
        return Err(VaultError::InvalidArtifact);
    }
    let vault_id = Uuid::from_bytes(take(&bytes, &mut offset)?);
    let artifact_id = Uuid::from_bytes(take(&bytes, &mut offset)?);
    if u32::from_le_bytes(take(&bytes, &mut offset)?) as usize != ARTIFACT_CHUNK_BYTES {
        return Err(VaultError::UnsupportedEnvelope);
    }
    let nonce_prefix = take(&bytes, &mut offset)?;
    if take::<1>(&bytes, &mut offset)?[0] != ARGON_ALGORITHM_ID {
        return Err(VaultError::UnsupportedEnvelope);
    }
    let argon_output_bytes = take::<1>(&bytes, &mut offset)?[0];
    let argon_memory_kib = u32::from_le_bytes(take(&bytes, &mut offset)?);
    let argon_iterations = u32::from_le_bytes(take(&bytes, &mut offset)?);
    let argon_lanes = u32::from_le_bytes(take(&bytes, &mut offset)?);
    let salt = take::<SALT_BYTES>(&bytes, &mut offset)?.to_vec();
    let nonce = take::<NONCE_BYTES>(&bytes, &mut offset)?.to_vec();
    let wrapped_master_key = take::<WRAPPED_KEY_BYTES>(&bytes, &mut offset)?.to_vec();
    let data_key_nonce = take(&bytes, &mut offset)?;
    let wrapped_data_key = take(&bytes, &mut offset)?;
    let recovery = RecoveryEnvelope {
        version: ENVELOPE_VERSION,
        vault_id,
        argon_algorithm: ARGON_ALGORITHM.to_owned(),
        argon_memory_kib,
        argon_iterations,
        argon_lanes,
        argon_output_bytes,
        salt,
        nonce,
        wrapped_master_key,
    };
    build_artifact_header(
        vault_id,
        artifact_id,
        recovery,
        nonce_prefix,
        data_key_nonce,
        wrapped_data_key,
    )
}

fn append<const N: usize>(buffer: &mut [u8; N], offset: &mut usize, value: &[u8]) {
    let end = *offset + value.len();
    buffer[*offset..end].copy_from_slice(value);
    *offset = end;
}

fn take<const N: usize>(bytes: &[u8], offset: &mut usize) -> Result<[u8; N], VaultError> {
    let end = offset.checked_add(N).ok_or(VaultError::InvalidArtifact)?;
    let value = bytes
        .get(*offset..end)
        .ok_or(VaultError::InvalidArtifact)?
        .try_into()
        .map_err(|_| VaultError::InvalidArtifact)?;
    *offset = end;
    Ok(value)
}

fn data_key_aad(vault_id: Uuid, artifact_id: Uuid) -> Vec<u8> {
    let mut aad = b"DBSUAL artifact data key v1\0".to_vec();
    aad.extend_from_slice(vault_id.as_bytes());
    aad.extend_from_slice(artifact_id.as_bytes());
    aad
}

fn chunk_aad(header_hash: &[u8; 32], artifact_id: Uuid, index: u64, length: u32) -> Vec<u8> {
    let mut aad = Vec::with_capacity(60);
    aad.extend_from_slice(header_hash);
    aad.extend_from_slice(artifact_id.as_bytes());
    aad.extend_from_slice(&index.to_le_bytes());
    aad.extend_from_slice(&length.to_le_bytes());
    aad
}

fn footer_aad(header_hash: &[u8; 32], artifact_id: Uuid) -> Vec<u8> {
    let mut aad = b"DBSUAL artifact end v1\0".to_vec();
    aad.extend_from_slice(header_hash);
    aad.extend_from_slice(artifact_id.as_bytes());
    aad
}

fn record_nonce(prefix: [u8; 4], index: u64) -> [u8; NONCE_BYTES] {
    let mut nonce = [0_u8; NONCE_BYTES];
    nonce[..4].copy_from_slice(&prefix);
    nonce[4..].copy_from_slice(&index.to_be_bytes());
    nonce
}

fn wrap_master_key(
    vault_id: Uuid,
    master_key: &MasterKey,
    phrase: &str,
) -> Result<RecoveryEnvelope, VaultError> {
    let normalized_phrase = normalized_mnemonic(phrase)?;
    let mut salt = [0_u8; SALT_BYTES];
    let mut nonce = [0_u8; NONCE_BYTES];
    random_bytes(&mut salt)?;
    random_bytes(&mut nonce)?;
    let mut wrapping_key = derive_wrapping_key(normalized_phrase.as_bytes(), &salt)?;
    let cipher =
        Aes256Gcm::new_from_slice(&wrapping_key.0).map_err(|_| VaultError::DerivationFailed)?;
    let nonce_value = Nonce::from(nonce);
    let wrapped_master_key = cipher
        .encrypt(
            &nonce_value,
            Payload {
                msg: &master_key.0,
                aad: &recovery_aad(vault_id),
            },
        )
        .map_err(|_| VaultError::AuthenticationFailed)?;
    wrapping_key.zeroize();

    Ok(RecoveryEnvelope {
        version: ENVELOPE_VERSION,
        vault_id,
        argon_algorithm: ARGON_ALGORITHM.to_owned(),
        argon_memory_kib: ARGON_MEMORY_KIB,
        argon_iterations: ARGON_ITERATIONS,
        argon_lanes: ARGON_LANES,
        argon_output_bytes: KEY_BYTES as u8,
        salt: salt.to_vec(),
        nonce: nonce.to_vec(),
        wrapped_master_key,
    })
}

fn normalized_mnemonic(phrase: &str) -> Result<Zeroizing<String>, VaultError> {
    if phrase.len() > 1024 {
        return Err(VaultError::InvalidRecoveryPhrase);
    }
    let mnemonic = Mnemonic::parse_in(Language::Spanish, phrase.trim())
        .map_err(|_| VaultError::InvalidRecoveryPhrase)?;
    if mnemonic.word_count() != 12 {
        return Err(VaultError::InvalidRecoveryPhrase);
    }
    Ok(Zeroizing::new(mnemonic.to_string()))
}

fn derive_wrapping_key(phrase: &[u8], salt: &[u8; SALT_BYTES]) -> Result<MasterKey, VaultError> {
    let params = Params::new(
        ARGON_MEMORY_KIB,
        ARGON_ITERATIONS,
        ARGON_LANES,
        Some(KEY_BYTES),
    )
    .map_err(|_| VaultError::DerivationFailed)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0_u8; KEY_BYTES]);
    argon
        .hash_password_into(phrase, salt, &mut *key)
        .map_err(|_| VaultError::DerivationFailed)?;
    let result = MasterKey(*key);
    Ok(result)
}

fn random_bytes(output: &mut [u8]) -> Result<(), VaultError> {
    getrandom::fill(output).map_err(|_| VaultError::RandomnessUnavailable)
}

fn recovery_aad(vault_id: Uuid) -> Vec<u8> {
    let mut aad = b"DBSUAL vault master key v1\0".to_vec();
    aad.extend_from_slice(vault_id.as_bytes());
    aad.push(ARGON_ALGORITHM_ID);
    aad.push(KEY_BYTES as u8);
    aad.extend_from_slice(&ARGON_MEMORY_KIB.to_le_bytes());
    aad.extend_from_slice(&ARGON_ITERATIONS.to_le_bytes());
    aad.extend_from_slice(&ARGON_LANES.to_le_bytes());
    aad
}

fn key_file_aad(vault_id: Uuid) -> Vec<u8> {
    let mut aad = b"DBSUAL vault key file v1\0".to_vec();
    aad.extend_from_slice(vault_id.as_bytes());
    aad.push(ARGON_ALGORITHM_ID);
    aad.push(KEY_BYTES as u8);
    aad.extend_from_slice(&ARGON_MEMORY_KIB.to_le_bytes());
    aad.extend_from_slice(&ARGON_ITERATIONS.to_le_bytes());
    aad.extend_from_slice(&ARGON_LANES.to_le_bytes());
    aad
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn encrypted_fixture() -> (NewVault, Vec<u8>) {
        let vault = create_vault().unwrap();
        let mut input = Cursor::new(vec![0x5a; ARTIFACT_CHUNK_BYTES * 2 + 17]);
        let mut output = Vec::new();
        encrypt_artifact(
            &mut input,
            &mut output,
            vault.vault_id,
            &vault.master_key,
            &vault.recovery_phrase,
        )
        .unwrap();
        (vault, output)
    }

    fn decrypt_fixture(
        bytes: &[u8],
        master_key: &MasterKey,
    ) -> (Result<ArtifactInfo, VaultError>, Vec<u8>) {
        let mut input = Cursor::new(bytes);
        let mut output = Vec::new();
        let result = read_artifact_header(&mut input).and_then(|header| {
            decrypt_artifact_with_master(&mut input, &mut output, header, master_key)
        });
        (result, output)
    }

    #[test]
    fn recovery_phrase_reopens_only_its_vault_key() {
        let vault = create_vault().unwrap();
        assert_eq!(vault.recovery_phrase.split_whitespace().count(), 12);
        assert_eq!(vault.recovery_envelope.vault_id, vault.vault_id);

        let recovered =
            recover_master_key(&vault.recovery_envelope, &vault.recovery_phrase).unwrap();
        assert_eq!(recovered.0, vault.master_key.0);
        let encoded = serde_json::to_vec(&vault.recovery_envelope).unwrap();
        assert!(!String::from_utf8_lossy(&encoded).contains(&vault.recovery_phrase));
        assert!(!encoded
            .windows(vault.master_key.0.len())
            .any(|window| window == vault.master_key.0));

        assert_eq!(
            recover_master_key(&vault.recovery_envelope, "palabra incorrecta").err(),
            Some(VaultError::InvalidRecoveryPhrase)
        );
    }

    #[test]
    fn imported_master_key_can_receive_a_new_recovery_phrase() {
        let portable = create_vault().unwrap();
        let vault_id = portable.vault_id;
        let expected_master = portable.master_key.0;
        let recovered = prepare_recovered_vault(vault_id, MasterKey(expected_master)).unwrap();
        assert_eq!(recovered.vault_id, vault_id);
        assert_eq!(recovered.master_key.0, expected_master);
        assert_eq!(recovered.recovery_phrase.split_whitespace().count(), 12);

        let opened =
            recover_master_key(&recovered.recovery_envelope, &recovered.recovery_phrase).unwrap();
        assert_eq!(opened.0, expected_master);
    }

    #[test]
    fn recovery_envelope_rejects_tampering() {
        let vault = create_vault().unwrap();
        let mut tampered = vault.recovery_envelope.clone();
        tampered.wrapped_master_key[0] ^= 1;
        assert_eq!(
            recover_master_key(&tampered, &vault.recovery_phrase).err(),
            Some(VaultError::AuthenticationFailed)
        );

        let mut rebound = vault.recovery_envelope.clone();
        rebound.vault_id = Uuid::new_v4();
        assert_eq!(
            recover_master_key(&rebound, &vault.recovery_phrase).err(),
            Some(VaultError::AuthenticationFailed)
        );

        let mut malformed = vault.recovery_envelope.clone();
        malformed.salt.clear();
        assert_eq!(
            recover_master_key(&malformed, &vault.recovery_phrase).err(),
            Some(VaultError::InvalidEnvelope)
        );

        let mut unsupported = vault.recovery_envelope.clone();
        unsupported.argon_memory_kib = u32::MAX;
        assert_eq!(
            recover_master_key(&unsupported, &vault.recovery_phrase).err(),
            Some(VaultError::UnsupportedEnvelope)
        );
    }

    #[test]
    fn recovery_phrase_normalizes_unicode_and_whitespace() {
        let vault = create_vault().unwrap();
        let spaced = format!("  {}  ", vault.recovery_phrase.replace(' ', "   "));
        let recovered = recover_master_key(&vault.recovery_envelope, &spaced).unwrap();
        assert_eq!(recovered.0, vault.master_key.0);
    }

    #[test]
    fn portable_key_file_round_trips_only_with_its_password() {
        let vault = create_vault().unwrap();
        let password = "Una-clave-portable-2026";
        let file = export_key_file(vault.vault_id, &vault.master_key, password).unwrap();
        assert!(!String::from_utf8_lossy(&file).contains(password));
        assert!(!file
            .windows(vault.master_key.0.len())
            .any(|window| window == vault.master_key.0));

        let (vault_id, recovered) = import_key_file(&file, password).unwrap();
        assert_eq!(vault_id, vault.vault_id);
        assert_eq!(recovered.0, vault.master_key.0);
        let mut plaintext = Cursor::new(b"contenido recuperado solo con archivo de clave".to_vec());
        let mut artifact = Vec::new();
        encrypt_artifact(
            &mut plaintext,
            &mut artifact,
            vault.vault_id,
            &vault.master_key,
            &vault.recovery_phrase,
        )
        .unwrap();
        let mut artifact_reader = Cursor::new(artifact);
        let mut recovered_contents = Vec::new();
        decrypt_artifact_with_key_file(
            &mut artifact_reader,
            &mut recovered_contents,
            &file,
            password,
        )
        .unwrap();
        assert_eq!(
            recovered_contents,
            b"contenido recuperado solo con archivo de clave"
        );
        assert_eq!(
            import_key_file(&file, "otra-clave-portable-2026").err(),
            Some(VaultError::AuthenticationFailed)
        );
        assert_eq!(
            export_key_file(vault.vault_id, &vault.master_key, "short").err(),
            Some(VaultError::WeakKeyFilePassword)
        );

        let mut damaged: serde_json::Value = serde_json::from_slice(&file).unwrap();
        damaged["wrappedMasterKey"][0] =
            serde_json::json!(damaged["wrappedMasterKey"][0].as_u64().unwrap() ^ 1);
        let damaged = serde_json::to_vec(&damaged).unwrap();
        assert_eq!(
            import_key_file(&damaged, password).err(),
            Some(VaultError::AuthenticationFailed)
        );

        let mut unsupported: serde_json::Value = serde_json::from_slice(&file).unwrap();
        unsupported["argonMemoryKib"] = serde_json::json!(u32::MAX);
        assert_eq!(
            import_key_file(&serde_json::to_vec(&unsupported).unwrap(), password).err(),
            Some(VaultError::UnsupportedEnvelope)
        );
    }

    #[test]
    fn portable_key_file_path_is_published_without_replacing_existing_files() {
        let vault = create_vault().unwrap();
        let directory =
            std::env::temp_dir().join(format!("dbsual-keyfile-test-{}", Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("recovery.dbsual-key");
        let password = "portable-key-password-2026";
        export_key_file_to_path(&path, vault.vault_id, &vault.master_key, password).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(import_key_file(&bytes, password).unwrap().0, vault.vault_id);
        assert_eq!(
            export_key_file_to_path(&path, vault.vault_id, &vault.master_key, password).err(),
            Some(VaultError::IoFailure)
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn artifact_stream_round_trips_large_input_in_bounded_chunks() {
        let (vault, artifact) = encrypted_fixture();
        assert_eq!(&artifact[..8], ARTIFACT_MAGIC);
        let (result, decrypted) = decrypt_fixture(&artifact, &vault.master_key);
        let info = result.unwrap();
        assert_eq!(info.vault_id, vault.vault_id);
        assert_eq!(info.plaintext_bytes, (ARTIFACT_CHUNK_BYTES * 2 + 17) as u64);
        assert_eq!(info.encrypted_chunks, 3);
        assert_eq!(decrypted, vec![0x5a; ARTIFACT_CHUNK_BYTES * 2 + 17]);
    }

    #[test]
    fn artifact_file_publishes_only_a_verified_ciphertext_package() {
        let vault = create_vault().unwrap();
        let directory = std::env::temp_dir().join(format!("dbsual-vault-test-{}", Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let source = directory.join("source.sql");
        let destination = directory.join("source.dbsual-artifact");
        let contents = b"INSERT INTO sample VALUES ('private fixture');".repeat(1200);
        std::fs::write(&source, &contents).unwrap();

        let info = encrypt_artifact_file(
            &source,
            &destination,
            vault.vault_id,
            &vault.master_key,
            &vault.recovery_phrase,
        )
        .unwrap();
        assert_eq!(info.plaintext_bytes, contents.len() as u64);
        assert!(std::fs::read_dir(&directory).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".partial")));
        let encrypted = std::fs::read(&destination).unwrap();
        assert!(!encrypted
            .windows(contents.len().min(32))
            .any(|window| window == &contents[..contents.len().min(32)]));
        assert_eq!(
            encrypt_artifact_file(
                &source,
                &destination,
                vault.vault_id,
                &vault.master_key,
                &vault.recovery_phrase,
            )
            .err(),
            Some(VaultError::IoFailure)
        );
        let (result, decrypted) = decrypt_fixture(&encrypted, &vault.master_key);
        assert_eq!(result.unwrap(), info);
        assert_eq!(decrypted, contents);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reader_stream_publishes_only_verified_ciphertext_without_plaintext_staging() {
        let vault = create_vault().unwrap();
        let directory = std::env::temp_dir().join(format!("dbsual-stream-{}", Uuid::new_v4()));
        let plain = vec![0x5a; ARTIFACT_CHUNK_BYTES * 3 + 17];
        let mut input = std::io::Cursor::new(plain.clone());
        let info = encrypt_artifact_reader_to_directory(
            &directory,
            &mut input,
            vault.vault_id,
            &vault.master_key,
            &vault.recovery_envelope,
        )
        .unwrap();
        let artifact = directory.join(format!("{}.dbsual-artifact", info.artifact.artifact_id));
        let encrypted = fs::read(&artifact).unwrap();
        assert!(!encrypted
            .windows(plain.len().min(64))
            .any(|part| part == &plain[..plain.len().min(64)]));
        assert_eq!(info.artifact.plaintext_bytes, plain.len() as u64);
        assert_eq!(info.artifact.encrypted_chunks, 4);
        assert_eq!(encrypted.len() as u64, info.encrypted_bytes);
        assert_eq!(
            Sha256::digest(&encrypted)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            info.ciphertext_sha256
        );
        let restored = directory.join("restored.tmp");
        decrypt_artifact_file(&artifact, &restored, &vault.recovery_phrase).unwrap();
        assert_eq!(fs::read(&restored).unwrap(), plain);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn interrupted_reader_leaves_no_published_or_partial_artifact() {
        struct InterruptedReader(Cursor<Vec<u8>>);
        impl Read for InterruptedReader {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.0.position() >= self.0.get_ref().len() as u64 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "simulated producer interruption",
                    ));
                }
                self.0.read(buffer)
            }
        }

        let vault = create_vault().unwrap();
        let directory = std::env::temp_dir().join(format!("dbsual-stream-fail-{}", Uuid::new_v4()));
        let mut input = InterruptedReader(Cursor::new(vec![0x33; ARTIFACT_CHUNK_BYTES]));
        let result = encrypt_artifact_reader_to_directory(
            &directory,
            &mut input,
            vault.vault_id,
            &vault.master_key,
            &vault.recovery_envelope,
        );
        assert!(result.is_err());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn sensitive_bytes_are_encrypted_before_they_reach_disk() {
        let vault = create_vault().unwrap();
        let directory = std::env::temp_dir().join(format!("dbsual-plan-{}", Uuid::new_v4()));
        let plaintext = b"UPDATE customers SET active = 0 WHERE id = 42;";
        let info = encrypt_artifact_bytes_to_file(
            &directory,
            plaintext,
            vault.vault_id,
            &vault.master_key,
            &vault.recovery_envelope,
        )
        .unwrap();
        let artifact_path = directory.join(format!("{}.dbsual-artifact", info.artifact_id));
        let encrypted = fs::read(&artifact_path).unwrap();
        assert!(!encrypted
            .windows(plaintext.len())
            .any(|window| window == plaintext));

        let restored_path = directory.join("restored.sql");
        decrypt_artifact_file(&artifact_path, &restored_path, &vault.recovery_phrase).unwrap();
        assert_eq!(fs::read(&restored_path).unwrap(), plaintext);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn staged_decryption_does_not_publish_partial_plaintext() {
        let vault = create_vault().unwrap();
        let directory =
            std::env::temp_dir().join(format!("dbsual-decrypt-test-{}", Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let source = directory.join("backup.dbsual");
        let destination = directory.join("restored.sql");
        let mut input = Cursor::new(b"sensitive database contents".to_vec());
        let mut encrypted = Vec::new();
        encrypt_artifact(
            &mut input,
            &mut encrypted,
            vault.vault_id,
            &vault.master_key,
            &vault.recovery_phrase,
        )
        .unwrap();
        let footer_tag = encrypted.len() - 1;
        encrypted[footer_tag] ^= 1;
        std::fs::write(&source, encrypted).unwrap();
        std::fs::write(&destination, b"existing destination").unwrap();

        assert_eq!(
            decrypt_artifact_file(&source, &destination, &vault.recovery_phrase).err(),
            Some(VaultError::AuthenticationFailed)
        );
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"existing destination"
        );
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
        std::fs::remove_file(&destination).unwrap();
        let mut valid_input = Cursor::new(b"sensitive database contents".to_vec());
        let mut valid = Vec::new();
        encrypt_artifact(
            &mut valid_input,
            &mut valid,
            vault.vault_id,
            &vault.master_key,
            &vault.recovery_phrase,
        )
        .unwrap();
        std::fs::write(&source, valid).unwrap();
        decrypt_artifact_file(&source, &destination, &vault.recovery_phrase).unwrap();
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"sensitive database contents"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn artifact_stream_rejects_tampering_reordering_truncation_and_trailing_data() {
        let (vault, artifact) = encrypted_fixture();
        let first_ciphertext = ARTIFACT_HEADER_BYTES + 8 + 4 + NONCE_BYTES;
        let mut changed = artifact.clone();
        changed[first_ciphertext] ^= 1;
        assert_eq!(
            decrypt_fixture(&changed, &vault.master_key).0,
            Err(VaultError::AuthenticationFailed)
        );

        let mut bad_length = artifact.clone();
        bad_length[ARTIFACT_HEADER_BYTES + 8..ARTIFACT_HEADER_BYTES + 12]
            .copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            decrypt_fixture(&bad_length, &vault.master_key).0,
            Err(VaultError::InvalidArtifact)
        );

        let mut bad_nonce = artifact.clone();
        bad_nonce[ARTIFACT_HEADER_BYTES + 12] ^= 1;
        assert_eq!(
            decrypt_fixture(&bad_nonce, &vault.master_key).0,
            Err(VaultError::InvalidArtifact)
        );

        let first_length = u32::from_le_bytes(
            artifact[ARTIFACT_HEADER_BYTES + 8..ARTIFACT_HEADER_BYTES + 12]
                .try_into()
                .unwrap(),
        ) as usize;
        let first_record_end = ARTIFACT_HEADER_BYTES + 8 + 4 + NONCE_BYTES + first_length + 16;
        let second_length = u32::from_le_bytes(
            artifact[first_record_end + 8..first_record_end + 12]
                .try_into()
                .unwrap(),
        ) as usize;
        let second_record_end = first_record_end + 8 + 4 + NONCE_BYTES + second_length + 16;
        let mut reordered = artifact[..ARTIFACT_HEADER_BYTES].to_vec();
        reordered.extend_from_slice(&artifact[first_record_end..second_record_end]);
        reordered.extend_from_slice(&artifact[ARTIFACT_HEADER_BYTES..first_record_end]);
        reordered.extend_from_slice(&artifact[second_record_end..]);
        assert_eq!(
            decrypt_fixture(&reordered, &vault.master_key).0,
            Err(VaultError::InvalidArtifact)
        );

        assert_eq!(
            decrypt_fixture(&artifact[..artifact.len() - 1], &vault.master_key).0,
            Err(VaultError::InvalidArtifact)
        );
        let mut bad_footer = artifact.clone();
        let footer_tag = bad_footer.len() - 1;
        bad_footer[footer_tag] ^= 1;
        assert_eq!(
            decrypt_fixture(&bad_footer, &vault.master_key).0,
            Err(VaultError::AuthenticationFailed)
        );
        let mut trailing = artifact.clone();
        trailing.push(0);
        assert_eq!(
            decrypt_fixture(&trailing, &vault.master_key).0,
            Err(VaultError::InvalidArtifact)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn keyring_verification_authenticates_the_complete_artifact_without_publishing_plaintext() {
        let vault = create_vault().unwrap();
        store_master_key(vault.vault_id, &vault.master_key).unwrap();
        let mut plaintext = Cursor::new(b"protected database stream".to_vec());
        let mut artifact = Vec::new();
        let expected = encrypt_artifact(
            &mut plaintext,
            &mut artifact,
            vault.vault_id,
            &vault.master_key,
            &vault.recovery_phrase,
        )
        .unwrap();

        let mut reader = Cursor::new(artifact.clone());
        assert_eq!(verify_artifact_from_keyring(&mut reader).unwrap(), expected);
        assert_eq!(reader.position(), artifact.len() as u64);

        let mut tampered = artifact;
        *tampered.last_mut().unwrap() ^= 1;
        let mut reader = Cursor::new(tampered);
        assert_eq!(
            verify_artifact_from_keyring(&mut reader).err(),
            Some(VaultError::AuthenticationFailed)
        );
        remove_master_key(vault.vault_id).unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_credential_store_round_trips_and_removes_ephemeral_vault_key() {
        let vault_id = Uuid::new_v4();
        let master_key = MasterKey([0x6d; KEY_BYTES]);
        store_master_key(vault_id, &master_key).unwrap();
        let loaded = load_master_key(vault_id).unwrap();
        assert_eq!(loaded.0, master_key.0);
        remove_master_key(vault_id).unwrap();
        assert_eq!(
            load_master_key(vault_id).err(),
            Some(VaultError::KeyringUnavailable)
        );
    }
}
