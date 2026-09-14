use crate::utils::get_vscode_version;

pub async fn fetch_vscode_version(client: &reqwest::Client) -> String {
    get_vscode_version(client).await
}
