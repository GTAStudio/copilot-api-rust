use crate::utils::get_vscode_version;

pub async fn fetch_vscode_version() -> String {
    get_vscode_version().await
}
