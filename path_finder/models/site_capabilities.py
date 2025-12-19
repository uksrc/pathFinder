import itertools
from pydantic import BaseModel, Field, TypeAdapter

# Type aliases to aid readability of model classes
SiteName = str
NodeName = str
StorageAreaID = str
SiteNameToStorageAreas = dict[SiteName, list["StorageArea"]]
NodeNameToSiteStorageAreas = dict[NodeName, SiteNameToStorageAreas]
StorageAreaIDToNodeAndSite = dict[StorageAreaID, tuple[NodeName, SiteName]]


class StorageArea(BaseModel):
    id: StorageAreaID
    name: str = Field(default="")
    type: str = Field(default="")
    relative_path: str = Field(default="")
    tier: int | None = Field(default=None)


class Storage(BaseModel):
    id: str
    name: str = Field(default="")
    areas: list[StorageArea]


class Site(BaseModel):
    id: str
    name: SiteName
    country: str
    storages: list[Storage]

    @property
    def storage_areas(self) -> list[StorageArea]:
        """Collate all storage areas from all storages in this site."""
        return list(
            itertools.chain.from_iterable(storage.areas for storage in self.storages)
        )


class Node(BaseModel):
    name: NodeName
    description: str = Field(default="")
    sites: list[Site]

    @property
    def storage_areas(self) -> SiteNameToStorageAreas:
        """Construct a mapping of site names to their storage areas."""
        return {site.name: [area for area in site.storage_areas] for site in self.sites}

    @property
    def storage_area_id_to_site_name(self) -> StorageAreaIDToNodeAndSite:
        """Construct a mapping of storage area IDs to their corresponding node and site names."""
        mapping: dict[str, tuple[NodeName, SiteName]] = {}
        for site_name, storage_areas in self.storage_areas.items():
            mapping.update({area.id: (self.name, site_name) for area in storage_areas})
        return mapping


# Define an entity which represents the API response containing a list of nodes
NodesAPIResponse = TypeAdapter(list[Node])


def get_all_node_storage_areas(nodes: list[Node]) -> StorageAreaIDToNodeAndSite:
    """Fetch all nodes and construct a mapping of storage area IDs to their corresponding node and site names.

    Returns:
      StorageAreaIDToNodeAndSite: A mapping of storage area IDs to their corresponding node
        and site names.
    """
    storage_area_mapping: StorageAreaIDToNodeAndSite = {}
    for node in nodes:
        storage_area_mapping.update(node.storage_area_id_to_site_name)
    return storage_area_mapping
