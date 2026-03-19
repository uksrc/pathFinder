use crate::models::*;
use anyhow::{Context, Result};
use reqwest::blocking::Client;

const DM_API_BASEURL: &str = "https://data-management.srcnet.skao.int/api/v1";
const SC_API_BASEURL: &str = "https://site-capabilities.srcnet.skao.int/api/v1";

pub struct ApiClient {
    client: Client,
    dm_token: String,
    sc_token: String,
}

impl ApiClient {
    pub fn new(dm_token: String, sc_token: String) -> Self {
        Self {
            client: Client::new(),
            dm_token,
            sc_token,
        }
    }

    pub fn check_namespace_available(&self, namespace: &str) -> Result<()> {
        let namespaces = self.get_all_namespaces()?;
        if !namespaces.contains(&namespace.to_string()) {
            anyhow::bail!(
                "Namespace '{}' not found in available namespaces: {:?}",
                namespace,
                namespaces
            );
        }
        Ok(())
    }

    pub fn get_all_namespaces(&self) -> Result<Vec<String>> {
        let url = format!("{}/data/list", DM_API_BASEURL);
        let response = self
            .client
            .get(&url)
            .bearer_auth(&self.dm_token)
            .send()
            .context("Failed to request namespaces from DM API")?;

        response
            .error_for_status()
            .context("DM API request failed")?
            .json()
            .context("Failed to parse namespaces response")
    }

    pub fn site_storage_areas(&self) -> Result<StorageAreaIDToNodeAndSite> {
        let url = format!("{}/nodes", SC_API_BASEURL);
        let response = self
            .client
            .get(&url)
            .bearer_auth(&self.sc_token)
            .send()
            .context("Failed to request nodes from SC API")?;

        let response = response
            .error_for_status()
            .context("SC API request failed")?;

        let response_text = response.text()
            .context("Failed to read response body")?;

        let nodes: NodesAPIResponse = serde_json::from_str(&response_text)
            .with_context(|| {
                format!(
                    "Failed to parse nodes response. Response body:\n{}",
                    if response_text.len() > 1000 {
                        format!("{}... (truncated)", &response_text[..1000])
                    } else {
                        response_text.clone()
                    }
                )
            })?;

        Ok(get_all_node_storage_areas(&nodes))
    }

    pub fn locate_data(&self, namespace: &str, file_name: &str) -> Result<DataLocationAPIResponse> {
        let url = format!("{}/data/locate/{}/{}", DM_API_BASEURL, namespace, file_name);
        let response = self
            .client
            .get(&url)
            .bearer_auth(&self.dm_token)
            .send()
            .with_context(|| {
                format!(
                    "Failed to locate file '{}' in namespace '{}' from DM API",
                    file_name, namespace
                )
            })?;

        let response = response
            .error_for_status()
            .context("DM API locate request failed")?;

        let response_text = response.text()
            .context("Failed to read response body")?;

        serde_json::from_str(&response_text)
            .with_context(|| {
                format!(
                    "Failed to parse data locations response. Response body:\n{}",
                    if response_text.len() > 1000 {
                        format!("{}... (truncated)", &response_text[..1000])
                    } else {
                        response_text.clone()
                    }
                )
            })
    }
}
