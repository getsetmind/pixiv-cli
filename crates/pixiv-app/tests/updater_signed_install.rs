mod support {
    pub mod updater_installer_contract;
    pub mod updater_windows_contract;
}

#[tokio::test]
async fn signed_installation_replays_frozen_go_contract_without_executing_candidates() {
    support::updater_installer_contract::replay_signed_install().await;
}

#[test]
fn source_detection_replays_frozen_metadata_and_filesystem_contract() {
    support::updater_installer_contract::replay_detectors();
}

#[test]
fn production_trust_is_the_frozen_public_key() {
    use pixiv_app::update::installer::{RELEASE_SIGNING_KEY_ID, RELEASE_SIGNING_PUBLIC_KEY};
    let fixture = support::updater_installer_contract::fixture();
    assert_eq!(RELEASE_SIGNING_KEY_ID, fixture["production_public_key_id"]);
    assert_eq!(
        support::updater_installer_contract::hex(&RELEASE_SIGNING_PUBLIC_KEY),
        fixture["production_public_key_hex"]
    );
}

#[test]
fn windows_replacement_replays_source_driven_api_contract_on_owned_unix_files() {
    support::updater_windows_contract::replay();
}

#[tokio::test]
async fn extra_archive_edges_follow_frozen_go_before_public_candidate_creation() {
    support::updater_installer_contract::replay_archive_edges().await;
}

#[tokio::test]
async fn sparse_archive_payloads_and_failures_follow_frozen_go() {
    support::updater_installer_contract::replay_sparse_archives().await;
}

#[tokio::test]
async fn old_gnu_sparse_archives_follow_frozen_go_validation() {
    support::updater_installer_contract::replay_old_gnu_archives().await;
}

#[tokio::test]
async fn historical_header_paths_and_numeric_fields_follow_frozen_go() {
    support::updater_installer_contract::replay_header_archives().await;
}

#[tokio::test]
async fn zip_creator_modes_follow_frozen_go() {
    support::updater_installer_contract::replay_zip_creator_archives().await;
}

#[tokio::test]
async fn zip_directory_count_cannot_hide_late_members() {
    support::updater_installer_contract::replay_zip_count_archives().await;
}
