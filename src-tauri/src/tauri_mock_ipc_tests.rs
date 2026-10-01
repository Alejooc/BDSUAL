use super::*;

/// Despacha el comando real de confirmación por MockRuntime y comprueba su validación IPC.
/// No aplica cambios en MySQL ni sustituye el recorrido de escritura con la app nativa.
pub fn confirm_history_revision_rejects_invalid_id_through_mock_tauri_ipc() {
    use tauri::test::{assert_ipc_response, mock_builder, mock_context, noop_assets, INVOKE_KEY};

    let app = mock_builder()
        .manage(AppState {
            storage: Err(AppError::storage()),
            artifacts_directory: std::env::temp_dir()
                .join(format!("dbsual-mock-artifacts-{}", uuid::Uuid::new_v4())),
            active: connections::ActiveConnections::default(),
            active_postgres: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            active_sqlite: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            active_read_queries: std::sync::Arc::default(),
            tunnels: connections::ActiveTunnels::default(),
            ssh_known_hosts_path: std::env::temp_dir().join("dbsual-mock-ssh-known-hosts"),
            vault_setup_lock: tokio::sync::Mutex::new(()),
            pending_vault: std::sync::Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![confirm_history_revision])
        .build(mock_context(noop_assets()))
        .expect("build isolated mock Tauri app");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("build mock IPC webview");

    assert_ipc_response(
        &webview,
        tauri::webview::InvokeRequest {
            cmd: "confirm_history_revision".into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(serde_json::json!({
                "revisionId": "not-a-uuid"
            })),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
        Err(serde_json::json!({
            "code": "INVALID_HISTORY_REVISION",
            "message": "La revisión del historial no es válida."
        })),
    );
}

/// Invoca el comando CSV real por IPC y comprueba que un separador inválido falla antes de acceder al almacenamiento.
pub fn prepare_mysql_csv_import_rejects_invalid_delimiter_through_mock_tauri_ipc() {
    use tauri::test::{assert_ipc_response, mock_builder, mock_context, noop_assets, INVOKE_KEY};

    let app = mock_builder()
        .manage(AppState {
            storage: Err(AppError::storage()),
            artifacts_directory: std::env::temp_dir()
                .join(format!("dbsual-mock-artifacts-{}", uuid::Uuid::new_v4())),
            active: connections::ActiveConnections::default(),
            active_postgres: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            active_sqlite: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            active_read_queries: std::sync::Arc::default(),
            tunnels: connections::ActiveTunnels::default(),
            ssh_known_hosts_path: std::env::temp_dir().join("dbsual-mock-ssh-known-hosts"),
            vault_setup_lock: tokio::sync::Mutex::new(()),
            pending_vault: std::sync::Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            crate::mysql_csv_batch::prepare_mysql_csv_import
        ])
        .build(mock_context(noop_assets()))
        .expect("build isolated mock Tauri app");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("build mock IPC webview");

    assert_ipc_response(
        &webview,
        tauri::webview::InvokeRequest {
            cmd: "prepare_mysql_csv_import".into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(serde_json::json!({
                "connectionId": "not-used-before-validation",
                "databaseName": "catalog",
                "tableName": "items",
                "csvText": "id,name\\n1,item",
                "delimiter": "pipe",
                "nullMarker": "\\N"
            })),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
        Err(serde_json::json!({
            "code": "CSV_IMPORT_INVALID",
            "message": "El CSV no cumple el formato o la tabla de destino."
        })),
    );
}

struct TestWorkspace(std::path::PathBuf);

impl TestWorkspace {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("dbsual-mock-ipc-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&path).expect("create isolated MockRuntime workspace");
        Self(path)
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct TestVaultKey(uuid::Uuid);

impl Drop for TestVaultKey {
    fn drop(&mut self) {
        let _ = crate::vault::remove_master_key(self.0);
    }
}

fn invoke_request(command: &str, args: serde_json::Value) -> tauri::webview::InvokeRequest {
    use tauri::test::INVOKE_KEY;
    tauri::webview::InvokeRequest {
        cmd: command.into(),
        callback: tauri::ipc::CallbackFn(0),
        error: tauri::ipc::CallbackFn(1),
        url: "http://tauri.localhost".parse().unwrap(),
        body: tauri::ipc::InvokeBody::Json(args),
        headers: Default::default(),
        invoke_key: INVOKE_KEY.to_string(),
    }
}

fn invoke_json<W: AsRef<tauri::Webview<tauri::test::MockRuntime>>>(
    webview: &W,
    command: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    tauri::test::get_ipc_response(webview, invoke_request(command, args))
        .unwrap_or_else(|error| {
            panic!("production IPC command `{command}` should succeed: {error:?}")
        })
        .deserialize::<serde_json::Value>()
        .expect("IPC command should return valid JSON")
}

/// Round-trips protected CSV commands through Tauri IPC and disposable MySQL.
pub async fn mysql_csv_import_and_compensation_round_trip_through_mock_tauri_ipc() {
    let pool = connections::connect_mysql_test_from_env()
        .await
        .expect("connect disposable MySQL server");
    let database = format!("dbsual_ipc_csv_{}", uuid::Uuid::new_v4().simple());
    let connection_id = uuid::Uuid::new_v4().to_string();
    let quote = |name: &str| format!("`{}`", name.replace('`', "``"));
    sqlx::query(&format!("CREATE DATABASE {}", quote(&database)))
        .execute(&pool)
        .await
        .expect("create disposable database");
    sqlx::query(&format!(
        "CREATE TABLE {}.items (id INT PRIMARY KEY, value VARCHAR(100) NULL) ENGINE=InnoDB",
        quote(&database)
    ))
    .execute(&pool)
    .await
    .expect("create fixture table");
    let workspace = TestWorkspace::new();
    let artifacts_directory = workspace.0.join("artifacts");
    std::fs::create_dir(&artifacts_directory).expect("create artifacts directory");
    let storage = crate::storage::open_at(workspace.0.join("history.sqlite"))
        .await
        .expect("open isolated history database");
    let vault = crate::vault::create_vault().expect("create isolated vault");
    crate::vault::store_master_key(vault.vault_id, &vault.master_key)
        .expect("store isolated test key in Windows Credential Manager");
    let _vault_key = TestVaultKey(vault.vault_id);
    crate::storage::save(&storage, crate::VAULT_ID_KEY, &vault.vault_id)
        .await
        .expect("save test vault ID");
    crate::storage::save(
        &storage,
        crate::VAULT_ENVELOPE_KEY,
        &vault.recovery_envelope,
    )
    .await
    .expect("save test recovery envelope");

    let state = AppState {
        storage: Ok(storage.clone()),
        artifacts_directory,
        active: connections::ActiveConnections::default(),
        active_postgres: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        active_sqlite: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        active_read_queries: std::sync::Arc::default(),
        tunnels: connections::ActiveTunnels::default(),
        ssh_known_hosts_path: workspace.0.join("ssh_known_hosts"),
        vault_setup_lock: tokio::sync::Mutex::new(()),
        pending_vault: std::sync::Mutex::new(None),
    };
    state
        .active
        .0
        .lock()
        .await
        .insert(connection_id.clone(), pool.clone());
    let app = tauri::test::mock_builder()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            crate::mysql_csv_batch::prepare_mysql_csv_import,
            crate::mysql_csv_batch::prepare_mysql_csv_import_revert,
            crate::mysql_row_change::apply_mysql_row_update,
            confirm_history_revision
        ])
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build isolated MockRuntime application");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("build MockRuntime webview");

    let initial_count: i64 =
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {}.items", quote(&database)))
            .fetch_one(&pool)
            .await
            .expect("count empty destination");
    assert_eq!(initial_count, 0);

    let prepared = invoke_json(
        &webview,
        "prepare_mysql_csv_import",
        serde_json::json!({
            "connectionId": connection_id,
            "databaseName": database,
            "tableName": "items",
            "csvText": "id,value\n1,one\n2,two\n",
            "delimiter": "comma",
            "nullMarker": "NULL"
        }),
    );
    assert_eq!(prepared["rowCount"], 2);
    let revision_id = prepared["revision"]["id"]
        .as_str()
        .expect("prepared revision ID")
        .to_owned();
    let count_after_prepare: i64 =
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {}.items", quote(&database)))
            .fetch_one(&pool)
            .await
            .expect("count before apply");
    assert_eq!(count_after_prepare, 0, "prepare must not write rows");

    let _ = invoke_json(
        &webview,
        "confirm_history_revision",
        serde_json::json!({ "revisionId": revision_id }),
    );
    let count_after_confirm: i64 =
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {}.items", quote(&database)))
            .fetch_one(&pool)
            .await
            .expect("count after confirm");
    assert_eq!(count_after_confirm, 0, "confirm must not write rows");

    let _ = invoke_json(
        &webview,
        "apply_mysql_row_update",
        serde_json::json!({ "revisionId": revision_id }),
    );
    let imported: Vec<(i32, String)> = sqlx::query_as(&format!(
        "SELECT id, value FROM {}.items ORDER BY id",
        quote(&database)
    ))
    .fetch_all(&pool)
    .await
    .expect("read imported rows");
    assert_eq!(imported, vec![(1, "one".into()), (2, "two".into())]);

    let prepared_revert = invoke_json(
        &webview,
        "prepare_mysql_csv_import_revert",
        serde_json::json!({ "revisionId": revision_id }),
    );
    assert_eq!(prepared_revert["rowCount"], 2);
    let revert_id = prepared_revert["revision"]["id"]
        .as_str()
        .expect("compensating revision ID")
        .to_owned();
    let _ = invoke_json(
        &webview,
        "confirm_history_revision",
        serde_json::json!({ "revisionId": revert_id }),
    );
    let before_revert_apply: i64 =
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {}.items", quote(&database)))
            .fetch_one(&pool)
            .await
            .expect("count before compensation apply");
    assert_eq!(before_revert_apply, 2);
    let _ = invoke_json(
        &webview,
        "apply_mysql_row_update",
        serde_json::json!({ "revisionId": revert_id }),
    );
    let count_after_revert: i64 =
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {}.items", quote(&database)))
            .fetch_one(&pool)
            .await
            .expect("count after compensation");
    assert_eq!(count_after_revert, 0);

    sqlx::query(&format!("DROP DATABASE {}", quote(&database)))
        .execute(&pool)
        .await
        .expect("drop disposable database");
}

/// Verifies row-edit review/apply/revert commands through production Tauri IPC.
pub async fn mysql_row_update_and_revert_round_trip_through_mock_tauri_ipc() {
    let pool = connections::connect_mysql_test_from_env()
        .await
        .expect("connect disposable MySQL server");
    let database = format!("dbsual_ipc_row_{}", uuid::Uuid::new_v4().simple());
    let connection_id = uuid::Uuid::new_v4().to_string();
    let quote = |name: &str| format!("`{}`", name.replace('`', "``"));
    sqlx::query(&format!("CREATE DATABASE {}", quote(&database)))
        .execute(&pool)
        .await
        .expect("create disposable database");
    sqlx::query(&format!(
        "CREATE TABLE {}.items (id INT PRIMARY KEY, value VARCHAR(100) NULL) ENGINE=InnoDB",
        quote(&database)
    ))
    .execute(&pool)
    .await
    .expect("create fixture table");
    sqlx::query(&format!(
        "INSERT INTO {}.items VALUES (1, 'before')",
        quote(&database)
    ))
    .execute(&pool)
    .await
    .expect("insert fixture row");

    let workspace = TestWorkspace::new();
    let artifacts_directory = workspace.0.join("artifacts");
    std::fs::create_dir(&artifacts_directory).expect("create artifacts directory");
    let storage = crate::storage::open_at(workspace.0.join("history.sqlite"))
        .await
        .expect("open isolated history database");
    let vault = crate::vault::create_vault().expect("create isolated vault");
    crate::vault::store_master_key(vault.vault_id, &vault.master_key)
        .expect("store isolated test key in Windows Credential Manager");
    let _vault_key = TestVaultKey(vault.vault_id);
    crate::storage::save(&storage, crate::VAULT_ID_KEY, &vault.vault_id)
        .await
        .expect("save test vault ID");
    crate::storage::save(
        &storage,
        crate::VAULT_ENVELOPE_KEY,
        &vault.recovery_envelope,
    )
    .await
    .expect("save test recovery envelope");

    let state = AppState {
        storage: Ok(storage.clone()),
        artifacts_directory,
        active: connections::ActiveConnections::default(),
        active_postgres: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        active_sqlite: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        active_read_queries: std::sync::Arc::default(),
        tunnels: connections::ActiveTunnels::default(),
        ssh_known_hosts_path: workspace.0.join("ssh_known_hosts"),
        vault_setup_lock: tokio::sync::Mutex::new(()),
        pending_vault: std::sync::Mutex::new(None),
    };
    state
        .active
        .0
        .lock()
        .await
        .insert(connection_id.clone(), pool.clone());
    let app = tauri::test::mock_builder()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            crate::mysql_row_change::prepare_mysql_row_update,
            crate::mysql_row_change::prepare_mysql_row_revert,
            crate::mysql_row_change::apply_mysql_row_update,
            confirm_history_revision
        ])
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build isolated MockRuntime application");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("build MockRuntime webview");
    let primary_key = serde_json::json!([{ "column": "id", "value": "1" }]);

    let prepared = invoke_json(
        &webview,
        "prepare_mysql_row_update",
        serde_json::json!({
            "connectionId": connection_id,
            "databaseName": database,
            "tableName": "items",
            "primaryKey": primary_key,
            "columnName": "value",
            "newValue": "after"
        }),
    );
    assert_eq!(prepared["oldValue"], "before");
    assert_eq!(prepared["newValue"], "after");
    let revision_id = prepared["revision"]["id"]
        .as_str()
        .expect("prepared revision ID")
        .to_owned();
    let read_value = || async {
        sqlx::query_scalar::<_, Option<String>>(&format!(
            "SELECT value FROM {}.items WHERE id = 1",
            quote(&database)
        ))
        .fetch_one(&pool)
        .await
        .expect("read target row")
    };
    assert_eq!(read_value().await.as_deref(), Some("before"));
    let _ = invoke_json(
        &webview,
        "confirm_history_revision",
        serde_json::json!({ "revisionId": revision_id }),
    );
    assert_eq!(read_value().await.as_deref(), Some("before"));
    let _ = invoke_json(
        &webview,
        "apply_mysql_row_update",
        serde_json::json!({ "revisionId": revision_id }),
    );
    assert_eq!(read_value().await.as_deref(), Some("after"));

    let prepared_revert = invoke_json(
        &webview,
        "prepare_mysql_row_revert",
        serde_json::json!({ "revisionId": revision_id }),
    );
    assert_eq!(prepared_revert["oldValue"], "after");
    assert_eq!(prepared_revert["newValue"], "before");
    let revert_id = prepared_revert["revision"]["id"]
        .as_str()
        .expect("compensating revision ID")
        .to_owned();
    let _ = invoke_json(
        &webview,
        "confirm_history_revision",
        serde_json::json!({ "revisionId": revert_id }),
    );
    assert_eq!(read_value().await.as_deref(), Some("after"));
    let _ = invoke_json(
        &webview,
        "apply_mysql_row_update",
        serde_json::json!({ "revisionId": revert_id }),
    );
    assert_eq!(read_value().await.as_deref(), Some("before"));

    sqlx::query(&format!("DROP DATABASE {}", quote(&database)))
        .execute(&pool)
        .await
        .expect("drop disposable database");
}
