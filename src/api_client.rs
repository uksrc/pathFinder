//! API client code for interacting with the SRCNet APIs

use crate::models::*;
use anyhow::{Context, Result};
use reqwest::blocking::Client;

const DM_API_BASEURL: &str = "https://data-management.srcnet.skao.int/api/v1";
const SC_API_BASEURL: &str = "https://site-capabilities.srcnet.skao.int/api/v1";

/// API client for interacting with the Path Finder APIs
///
/// This trait allows for abstraction and easier testing of API interactions.
/// The `ApiClient` struct provides a concrete implementation.
pub trait PathFinderApiClient {

    /// Checks if the specified namespace is available by querying the DM API.
    fn check_namespace_available(&self, namespace: &str) -> Result<()>;

    /// Retrieves a list of all available namespaces from the DM API.
    fn get_all_namespaces(&self) -> Result<Vec<String>>;

    /// Retrieves a mapping of storage area IDs to their associated node and site information from the SC API.
    fn site_storage_areas(&self) -> Result<StorageAreaIDToNodeAndSite>;

    /// Locates the specified data file within the given namespace by querying the DM API.
    fn locate_data(&self, namespace: &str, file_name: &str) -> Result<DataLocationAPIResponse>;
}

pub struct ApiClient {
    client: Client,
    dm_token: String,
    sc_token: String,
    dm_base_url: String,
    sc_base_url: String,
}

impl ApiClient {
    pub fn new(dm_token: String, sc_token: String) -> Self {
        Self {
            client: Client::new(),
            dm_token,
            sc_token,
            dm_base_url: DM_API_BASEURL.to_string(),
            sc_base_url: SC_API_BASEURL.to_string(),
        }
    }

    #[cfg(test)]
    pub fn new_with_urls(
        dm_token: String,
        sc_token: String,
        dm_base_url: String,
        sc_base_url: String,
    ) -> Self {
        Self {
            client: Client::new(),
            dm_token,
            sc_token,
            dm_base_url,
            sc_base_url,
        }
    }
}

/// Implementation of the `PathFinderApiClient` trait for `ApiClient`, providing concrete logic for API interactions.
///
/// See the trait for method documentation.
impl PathFinderApiClient for ApiClient {
    fn check_namespace_available(&self, namespace: &str) -> Result<()> {
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

    fn get_all_namespaces(&self) -> Result<Vec<String>> {
        let url = format!("{}/data/list", self.dm_base_url);
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

    fn site_storage_areas(&self) -> Result<StorageAreaIDToNodeAndSite> {
        let url = format!("{}/nodes", self.sc_base_url);
        let response = self
            .client
            .get(&url)
            .bearer_auth(&self.sc_token)
            .send()
            .context("Failed to request nodes from SC API")?;

        let response = response
            .error_for_status()
            .context("SC API request failed")?;

        let response_text = response.text().context("Failed to read response body")?;

        let nodes: NodesAPIResponse = serde_json::from_str(&response_text).with_context(|| {
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

    fn locate_data(&self, namespace: &str, file_name: &str) -> Result<DataLocationAPIResponse> {
        let url = format!(
            "{}/data/locate/{}/{}",
            self.dm_base_url, namespace, file_name
        );
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

        let response_text = response.text().context("Failed to read response body")?;

        serde_json::from_str(&response_text).with_context(|| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    fn client_for(dm_server: &MockServer, sc_server: &MockServer) -> ApiClient {
        ApiClient::new_with_urls(
            "dm-token".to_string(),
            "sc-token".to_string(),
            dm_server.base_url(),
            sc_server.base_url(),
        )
    }

    // --- get_all_namespaces ---

    #[test]
    fn get_all_namespaces_returns_parsed_list() {
        let dm = MockServer::start();
        let sc = MockServer::start();
        dm.mock(|when, then| {
            when.method(GET).path("/data/list");
            then.status(200).body(r#"["daac","lsst","ska-mid"]"#);
        });

        let namespaces = client_for(&dm, &sc).get_all_namespaces().unwrap();
        assert_eq!(namespaces, vec!["daac", "lsst", "ska-mid"]);
    }

    #[test]
    fn get_all_namespaces_propagates_401() {
        let dm = MockServer::start();
        let sc = MockServer::start();
        dm.mock(|when, then| {
            when.method(GET).path("/data/list");
            then.status(401).body("Unauthorized");
        });

        let err = client_for(&dm, &sc).get_all_namespaces().unwrap_err();
        assert!(err.to_string().contains("DM API request failed"), "{err}");
    }

    #[test]
    fn get_all_namespaces_propagates_500() {
        let dm = MockServer::start();
        let sc = MockServer::start();
        dm.mock(|when, then| {
            when.method(GET).path("/data/list");
            then.status(500).body("Internal Server Error");
        });

        assert!(client_for(&dm, &sc).get_all_namespaces().is_err());
    }

    // --- check_namespace_available ---

    #[test]
    fn check_namespace_available_succeeds_when_present() {
        let dm = MockServer::start();
        let sc = MockServer::start();
        dm.mock(|when, then| {
            when.method(GET).path("/data/list");
            then.status(200).body(r#"["daac","lsst"]"#);
        });

        assert!(client_for(&dm, &sc)
            .check_namespace_available("daac")
            .is_ok());
    }

    #[test]
    fn check_namespace_available_bails_when_absent() {
        let dm = MockServer::start();
        let sc = MockServer::start();
        dm.mock(|when, then| {
            when.method(GET).path("/data/list");
            then.status(200).body(r#"["lsst"]"#);
        });

        let err = client_for(&dm, &sc)
            .check_namespace_available("daac")
            .unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
    }

    // --- site_storage_areas ---

    #[test]
    fn site_storage_areas_empty_nodes_returns_empty_map() {
        let dm = MockServer::start();
        let sc = MockServer::start();
        sc.mock(|when, then| {
            when.method(GET).path("/nodes");
            then.status(200).body("[]");
        });

        let map = client_for(&dm, &sc).site_storage_areas().unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn site_storage_areas_parses_node_storage_mapping() {
        let dm = MockServer::start();
        let sc = MockServer::start();
        sc.mock(|when, then| {
            when.method(GET).path("/nodes");
            then.status(200).body(
                r#"[
                {
                    "name": "uk-node",
                    "description": "UK Node",
                    "sites": [{
                        "id": "site-1",
                        "name": "Oxford",
                        "country": "GB",
                        "storages": [{
                            "id": "storage-1",
                            "name": "primary",
                            "areas": [{
                                "id": "area-abc",
                                "name": "fits-store",
                                "type": "disk",
                                "relative_path": "/data",
                                "tier": 1
                            }]
                        }]
                    }]
                }
            ]"#,
            );
        });

        let map = client_for(&dm, &sc).site_storage_areas().unwrap();
        assert!(map.contains_key("area-abc"));
        let (node, site, area) = map.get("area-abc").unwrap();
        assert_eq!(node, "uk-node");
        assert_eq!(site, "Oxford");
        assert_eq!(area, "fits-store");
    }

    #[test]
    fn site_storage_areas_propagates_401() {
        let dm = MockServer::start();
        let sc = MockServer::start();
        sc.mock(|when, then| {
            when.method(GET).path("/nodes");
            then.status(401);
        });

        let err = client_for(&dm, &sc).site_storage_areas().unwrap_err();
        assert!(err.to_string().contains("SC API request failed"), "{err}");
    }

    #[test]
    fn site_storage_areas_errors_on_malformed_json() {
        let dm = MockServer::start();
        let sc = MockServer::start();
        sc.mock(|when, then| {
            when.method(GET).path("/nodes");
            then.status(200).body("not json at all");
        });

        let err = client_for(&dm, &sc).site_storage_areas().unwrap_err();
        assert!(
            err.to_string().contains("Failed to parse nodes response"),
            "{err}"
        );
    }

    // --- locate_data ---

    #[test]
    fn locate_data_returns_parsed_locations() {
        let dm = MockServer::start();
        let sc = MockServer::start();
        dm.mock(|when, then| {
            when.method(GET).path("/data/locate/daac/file.fits");
            then.status(200).body(
                r#"[
                {
                    "identifier": "loc-1",
                    "associated_storage_area_id": "area-abc",
                    "replicas": ["rucio://rse1/daac/2022/file.fits"]
                }
            ]"#,
            );
        });

        let locations = client_for(&dm, &sc)
            .locate_data("daac", "file.fits")
            .unwrap();
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].identifier, "loc-1");
        assert_eq!(locations[0].replicas[0], "rucio://rse1/daac/2022/file.fits");
    }

    #[test]
    fn locate_data_returns_empty_list() {
        let dm = MockServer::start();
        let sc = MockServer::start();
        dm.mock(|when, then| {
            when.method(GET).path("/data/locate/daac/missing.fits");
            then.status(200).body("[]");
        });

        let locations = client_for(&dm, &sc)
            .locate_data("daac", "missing.fits")
            .unwrap();
        assert!(locations.is_empty());
    }

    #[test]
    fn locate_data_propagates_404() {
        let dm = MockServer::start();
        let sc = MockServer::start();
        dm.mock(|when, then| {
            when.method(GET).path("/data/locate/daac/file.fits");
            then.status(404);
        });

        assert!(client_for(&dm, &sc)
            .locate_data("daac", "file.fits")
            .is_err());
    }

    #[test]
    fn locate_data_errors_on_malformed_json() {
        let dm = MockServer::start();
        let sc = MockServer::start();
        dm.mock(|when, then| {
            when.method(GET).path("/data/locate/daac/file.fits");
            then.status(200).body("{bad json}");
        });

        let err = client_for(&dm, &sc)
            .locate_data("daac", "file.fits")
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("Failed to parse data locations response"),
            "{err}"
        );
    }
}
