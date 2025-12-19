from pydantic import BaseModel, TypeAdapter


class DataLocation(BaseModel):
    identifier: str
    associated_storage_area_id: str
    replicas: list[str]


DataLocationAPIResponse = TypeAdapter(list[DataLocation])
