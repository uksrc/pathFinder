use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataLocation {
    pub identifier: String,
    pub associated_storage_area_id: String,
    pub replicas: Vec<String>,
}

pub type DataLocationAPIResponse = Vec<DataLocation>;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Storage {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub areas: Vec<StorageArea>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Site {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub storages: Vec<Storage>,
}

impl Site {
    pub fn storage_areas(&self) -> Vec<&StorageArea> {
        self.storages
            .iter()
            .flat_map(|storage| storage.areas.iter())
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub sites: Vec<Site>,
}

impl Node {
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

pub type NodesAPIResponse = Vec<Node>;
pub type StorageAreaIDToNodeAndSite = HashMap<String, (String, String, String)>;

pub fn get_all_node_storage_areas(nodes: &[Node]) -> StorageAreaIDToNodeAndSite {
    let mut storage_area_mapping = HashMap::new();
    for node in nodes {
        storage_area_mapping.extend(node.storage_area_id_to_site_name());
    }
    storage_area_mapping
}
