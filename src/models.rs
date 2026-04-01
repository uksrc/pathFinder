//! Data models for the SRCNet APIs

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single data location returned by the DM API, describing where a file replica or replicas live.
///
/// Example:
///
/// {
///   "identifier": "MARSSRC-OLYMPUSMONS-T0",
///   "associated_storage_area_id": "2a73d212-8793-4011-a687-cad99841c269",
///   "replicas": [
///     "davs://xrootd01.olympusmons.marssrc.org:1094/skadata/daac/08/06/random10MiB.bin"
///   ],
///   "is_dataset": false
/// }
///
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataLocation {
    pub identifier: String,
    pub associated_storage_area_id: String,
    pub replicas: Vec<String>,
    pub is_dataset: bool,
}

/// The full response from the DM API locate endpoint: a list of [`DataLocation`] entries.
pub type DataLocationAPIResponse = Vec<DataLocation>;

/// A storage area within a [`Storage`] resource at a site.
///
/// Default values are required as some fields are not populated in the API response.
///
/// Example:
/// {
///   "id": "ce04d165-4d5f-4380-a674-2a9ae4aba75e",
///   "type": "rse",
///   "relative_path": "/",
///   "name": "MARSSRC_VALLESMARINERIS_XRD",
///   "tier": 1
/// }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageArea {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(rename = "type", default)]
    pub storage_type: String,
    #[serde(default)]
    pub relative_path: String,
    pub tier: Option<i32>,
}

/// A storage resource at a site, containing one or more [`StorageArea`]s.
///
/// Default values are required as some fields are not populated in the API response.
///
/// Example:
/// {
///   "id": "12345678-90ab-cdef-1234-567890abcdef",
///   "host": "myxrootd.example.com",
///   "base_path": "/base/data/",
///   "srm": "xrd",
///   "device_type": "hdd",
///   "size_in_terabytes": 200,
///   "name": "MARSSRC_VALLESMARINERIS_XRD",
///   "supported_protocols": [
///     {
///       "prefix": "https",
///       "port": 1094
///     }
///   ],
///   "downtime": [
///     {
///       "id": "12345678-90ab-cdef-1234-567890abcdef",
///       "date_range": "2099-03-15T12:00:00.000Z to 2099-04-01T11:59:59.999Z",
///       "type": "Planned",
///       "reason": "Beware the Ides of March! Don't be a fool!"
///     }
///   ],
///   "areas": [
///     ...
///   ]
/// }
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Storage {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub areas: Vec<StorageArea>,
    // ... other fields omitted
}

/// A physical site belonging to a [`Node`], containing one or more [`Storage`] resources.
///
/// Default values are required as some fields are not populated in the API response.
///
/// Example:
/// {
///       "id": "12345678-90ab-cdef-1234-567890abcdef",
///       "name": "MARSSRC-VALLESMARINERIS",
///       "description": "Rutherford Appleton Laboratory",
///       "country": "GB",
///       "latitude": 51.5707,
///       "longitude": -1.3088,
///       "primary_contact_email": "onna@example.com ",
///       "secondary_contact_email": "otoko@example.com",
///       "storages": [
///         ...
///       ],
///       "compute": [
///         ...
///       ]
///     }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Site {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub storages: Vec<Storage>,
    // ... other fields omitted
}

impl Site {
    /// Returns a flat list of all [`StorageArea`]s across every [`Storage`] at this site.
    pub fn storage_areas(&self) -> Vec<&StorageArea> {
        self.storages
            .iter()
            .flat_map(|storage| storage.areas.iter())
            .collect()
    }
}

/// An SRCNet node, as returned from the SC API /nodes endpoint, grouping one or more [`Site`]s under a common name.
///
/// Default values are required as some fields are not populated in the API response.
///
/// Example:
/// {
///   "name": "MARSSRC",
///   "description": "MARSSRC Node",
///   "sites": [
///     ...
///   ],
///   "last_updated_at": "2026-03-19T14:24:37.869190",
///   "last_updated_by_username": "ma2223",
///   "version": 50
/// }
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub sites: Vec<Site>,
    // ... other fields omitted
}

impl Node {
    /// Builds a map from storage area ID to a `(node_name, site_name, area_name)` tuple
    /// for every storage area across all sites in this node.
    pub fn storage_area_id_to_site_name(&self) -> HashMap<String, (String, String, String)> {
        let mut mapping = HashMap::new();
        for site in &self.sites {
            for area in site.storage_areas() {
                mapping.insert(
                    area.id.clone(),
                    (self.name.clone(), site.name.clone(), area.name.clone()),
                );
            }
        }
        mapping
    }
}

/// The full response from the SC API nodes endpoint: a list of [`Node`]s.
pub type NodesAPIResponse = Vec<Node>;

/// A map from storage area ID to a `(node_name, site_name, area_name)` tuple,
/// aggregated across all nodes.
pub type StorageAreaIDToNodeAndSite = HashMap<String, (String, String, String)>;

/// Aggregates storage area mappings across all provided nodes into a single
/// [`StorageAreaIDToNodeAndSite`] map.
pub fn get_all_node_storage_areas(nodes: &[Node]) -> StorageAreaIDToNodeAndSite {
    let mut storage_area_mapping = HashMap::new();
    for node in nodes {
        storage_area_mapping.extend(node.storage_area_id_to_site_name());
    }
    storage_area_mapping
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALLESMARINERIS_AREA_ID: &str = "ce04d165-4d5f-4380-a674-2a9ae4aba75e";
    const OLYMPUSMONS_AREA_ID: &str = "2a73d212-8793-4011-a687-cad99841c269";
    const VALLESMARINERIS_SITE_ID: &str = "a1b2c3d4-e5f6-7890-abcd-ef1234567890";
    const OLYMPUSMONS_SITE_ID: &str = "b2c3d4e5-f6a7-8901-bcde-f12345678901";

    // --- helpers ---

    fn make_area(id: &str, name: &str) -> StorageArea {
        StorageArea {
            id: id.to_string(),
            name: name.to_string(),
            storage_type: "rse".to_string(),
            relative_path: "/".to_string(),
            tier: Some(1),
        }
    }

    fn make_storage(id: &str, name: &str, areas: Vec<StorageArea>) -> Storage {
        Storage {
            id: id.to_string(),
            name: name.to_string(),
            areas,
        }
    }

    fn make_site(id: &str, name: &str, storages: Vec<Storage>) -> Site {
        Site {
            id: id.to_string(),
            name: name.to_string(),
            country: "GB".to_string(),
            storages,
        }
    }

    fn make_node(name: &str, sites: Vec<Site>) -> Node {
        Node {
            name: name.to_string(),
            description: format!("{} Node", name),
            sites,
        }
    }

    // --- Site::storage_areas ---

    #[test]
    fn site_storage_areas_empty_storages_returns_empty() {
        let site = make_site(VALLESMARINERIS_SITE_ID, "MARSSRC-VALLESMARINERIS", vec![]);
        assert!(site.storage_areas().is_empty());
    }

    #[test]
    fn site_storage_areas_flattens_multiple_storages() {
        let site = make_site(
            VALLESMARINERIS_SITE_ID,
            "MARSSRC-VALLESMARINERIS",
            vec![
                make_storage(
                    "st1",
                    "MARSSRC_VALLESMARINERIS_XRD",
                    vec![make_area(
                        VALLESMARINERIS_AREA_ID,
                        "MARSSRC_VALLESMARINERIS_XRD",
                    )],
                ),
                make_storage(
                    "st2",
                    "MARSSRC_VALLESMARINERIS_STORM",
                    vec![
                        make_area(OLYMPUSMONS_AREA_ID, "MARSSRC_VALLESMARINERIS_STORM"),
                        make_area(
                            "c3d4e5f6-a7b8-9012-cdef-123456789012",
                            "MARSSRC_VALLESMARINERIS_TAPE",
                        ),
                    ],
                ),
            ],
        );
        let areas = site.storage_areas();
        assert_eq!(areas.len(), 3);
        let ids: Vec<&str> = areas.iter().map(|a| a.id.as_str()).collect();
        assert!(ids.contains(&VALLESMARINERIS_AREA_ID));
        assert!(ids.contains(&OLYMPUSMONS_AREA_ID));
        assert!(ids.contains(&"c3d4e5f6-a7b8-9012-cdef-123456789012"));
    }

    // --- Node::storage_area_id_to_site_name ---

    #[test]
    fn storage_area_id_to_site_name_empty_sites_returns_empty() {
        let node = make_node("MARSSRC", vec![]);
        assert!(node.storage_area_id_to_site_name().is_empty());
    }

    #[test]
    fn storage_area_id_to_site_name_maps_correctly() {
        let node = make_node(
            "MARSSRC",
            vec![make_site(
                VALLESMARINERIS_SITE_ID,
                "MARSSRC-VALLESMARINERIS",
                vec![make_storage(
                    "st1",
                    "MARSSRC_VALLESMARINERIS_XRD",
                    vec![make_area(
                        VALLESMARINERIS_AREA_ID,
                        "MARSSRC_VALLESMARINERIS_XRD",
                    )],
                )],
            )],
        );
        let map = node.storage_area_id_to_site_name();
        assert_eq!(map.len(), 1);
        let (node_name, site_name, area_name) = map.get(VALLESMARINERIS_AREA_ID).unwrap();
        assert_eq!(node_name, "MARSSRC");
        assert_eq!(site_name, "MARSSRC-VALLESMARINERIS");
        assert_eq!(area_name, "MARSSRC_VALLESMARINERIS_XRD");
    }

    #[test]
    fn storage_area_id_to_site_name_multiple_sites() {
        let node = make_node(
            "MARSSRC",
            vec![
                make_site(
                    VALLESMARINERIS_SITE_ID,
                    "MARSSRC-VALLESMARINERIS",
                    vec![make_storage(
                        "st1",
                        "MARSSRC_VALLESMARINERIS_XRD",
                        vec![make_area(
                            VALLESMARINERIS_AREA_ID,
                            "MARSSRC_VALLESMARINERIS_XRD",
                        )],
                    )],
                ),
                make_site(
                    OLYMPUSMONS_SITE_ID,
                    "MARSSRC-OLYMPUSMONS",
                    vec![make_storage(
                        "st2",
                        "MARSSRC_OLYMPUSMONS_XRD",
                        vec![make_area(OLYMPUSMONS_AREA_ID, "MARSSRC_OLYMPUSMONS_XRD")],
                    )],
                ),
            ],
        );
        let map = node.storage_area_id_to_site_name();
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get(VALLESMARINERIS_AREA_ID).unwrap().1,
            "MARSSRC-VALLESMARINERIS"
        );
        assert_eq!(
            map.get(OLYMPUSMONS_AREA_ID).unwrap().1,
            "MARSSRC-OLYMPUSMONS"
        );
    }

    // --- get_all_node_storage_areas ---

    #[test]
    fn get_all_node_storage_areas_empty_nodes_returns_empty() {
        let map = get_all_node_storage_areas(&[]);
        assert!(map.is_empty());
    }

    #[test]
    fn get_all_node_storage_areas_aggregates_across_nodes() {
        let aussrc_area_id = "d4e5f6a7-b8c9-0123-defa-234567890123";
        let nodes = vec![
            make_node(
                "MARSSRC",
                vec![make_site(
                    VALLESMARINERIS_SITE_ID,
                    "MARSSRC-VALLESMARINERIS",
                    vec![make_storage(
                        "st1",
                        "MARSSRC_VALLESMARINERIS_XRD",
                        vec![make_area(
                            VALLESMARINERIS_AREA_ID,
                            "MARSSRC_VALLESMARINERIS_XRD",
                        )],
                    )],
                )],
            ),
            make_node(
                "AUSSRC",
                vec![make_site(
                    "e5f6a7b8-c9d0-1234-efab-345678901234",
                    "AUSSRC-ICRAR",
                    vec![make_storage(
                        "st2",
                        "AUSSRC_ICRAR_XRD",
                        vec![make_area(aussrc_area_id, "AUSSRC_ICRAR_XRD")],
                    )],
                )],
            ),
        ];
        let map = get_all_node_storage_areas(&nodes);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(VALLESMARINERIS_AREA_ID).unwrap().0, "MARSSRC");
        assert_eq!(map.get(aussrc_area_id).unwrap().0, "AUSSRC");
    }

    #[test]
    fn get_all_node_storage_areas_later_node_wins_on_duplicate_id() {
        let nodes = vec![
            make_node(
                "MARSSRC",
                vec![make_site(
                    VALLESMARINERIS_SITE_ID,
                    "MARSSRC-VALLESMARINERIS",
                    vec![make_storage(
                        "st1",
                        "MARSSRC_VALLESMARINERIS_XRD",
                        vec![make_area(
                            VALLESMARINERIS_AREA_ID,
                            "MARSSRC_VALLESMARINERIS_XRD",
                        )],
                    )],
                )],
            ),
            make_node(
                "AUSSRC",
                vec![make_site(
                    "e5f6a7b8-c9d0-1234-efab-345678901234",
                    "AUSSRC-ICRAR",
                    vec![make_storage(
                        "st2",
                        "AUSSRC_ICRAR_XRD",
                        vec![make_area(VALLESMARINERIS_AREA_ID, "AUSSRC_ICRAR_XRD")],
                    )],
                )],
            ),
        ];
        let map = get_all_node_storage_areas(&nodes);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(VALLESMARINERIS_AREA_ID).unwrap().0, "AUSSRC");
    }

    // --- DataLocation deserialisation ---

    #[test]
    fn data_location_deserialises_from_json() {
        let json = r#"{
            "identifier": "MARSSRC-OLYMPUSMONS-T0",
            "associated_storage_area_id": "2a73d212-8793-4011-a687-cad99841c269",
            "replicas": ["davs://xrootd01.olympusmons.marssrc.org:1094/skadata/daac/08/06/random10MiB.bin"],
            "is_dataset": false
        }"#;
        let loc: DataLocation = serde_json::from_str(json).unwrap();
        assert_eq!(loc.identifier, "MARSSRC-OLYMPUSMONS-T0");
        assert_eq!(
            loc.associated_storage_area_id,
            "2a73d212-8793-4011-a687-cad99841c269"
        );
        assert_eq!(
            loc.replicas[0],
            "davs://xrootd01.olympusmons.marssrc.org:1094/skadata/daac/08/06/random10MiB.bin"
        );
        assert!(!loc.is_dataset);
    }

    // --- StorageArea deserialisation ---

    #[test]
    fn storage_area_deserialises_from_json() {
        let json = r#"{
            "id": "ce04d165-4d5f-4380-a674-2a9ae4aba75e",
            "type": "rse",
            "relative_path": "/",
            "name": "MARSSRC_VALLESMARINERIS_XRD",
            "tier": 1
        }"#;
        let area: StorageArea = serde_json::from_str(json).unwrap();
        assert_eq!(area.id, "ce04d165-4d5f-4380-a674-2a9ae4aba75e");
        assert_eq!(area.storage_type, "rse");
        assert_eq!(area.relative_path, "/");
        assert_eq!(area.name, "MARSSRC_VALLESMARINERIS_XRD");
        assert_eq!(area.tier, Some(1));
    }

    #[test]
    fn storage_area_defaults_missing_optional_fields() {
        let json = r#"{"id": "ce04d165-4d5f-4380-a674-2a9ae4aba75e"}"#;
        let area: StorageArea = serde_json::from_str(json).unwrap();
        assert_eq!(area.id, "ce04d165-4d5f-4380-a674-2a9ae4aba75e");
        assert_eq!(area.name, "");
        assert_eq!(area.storage_type, "");
        assert_eq!(area.relative_path, "");
        assert!(area.tier.is_none());
    }
}
