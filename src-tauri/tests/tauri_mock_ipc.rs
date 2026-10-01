#[test]
fn confirm_history_revision_rejects_invalid_id_through_mock_tauri_ipc() {
    dbsual_lib::tauri_mock_ipc_tests::confirm_history_revision_rejects_invalid_id_through_mock_tauri_ipc();
}

#[test]
fn prepare_mysql_csv_import_rejects_invalid_delimiter_through_mock_tauri_ipc() {
    dbsual_lib::tauri_mock_ipc_tests::prepare_mysql_csv_import_rejects_invalid_delimiter_through_mock_tauri_ipc();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires DBSUAL_MYSQL_TEST_PASSWORD and disposable MySQL"]
async fn mysql_csv_import_and_compensation_round_trip_through_mock_tauri_ipc() {
    dbsual_lib::tauri_mock_ipc_tests::mysql_csv_import_and_compensation_round_trip_through_mock_tauri_ipc().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requiere DBSUAL_MYSQL_TEST_PASSWORD y MySQL desechable"]
async fn mysql_row_update_and_revert_round_trip_through_mock_tauri_ipc() {
    dbsual_lib::tauri_mock_ipc_tests::mysql_row_update_and_revert_round_trip_through_mock_tauri_ipc().await;
}
