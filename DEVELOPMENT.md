# Architecture


## Features

- OAuth2 device code flow authentication
- Data location lookup via Data Management API
- Site capabilities verification via Site Capabilities API
- Secure data mounting with proper permissions


### Modules

- **main.rs** - Main path finder CLI logic
- **api_client.rs** - HTTP client for Data Management and Site Capabilities APIs
- **oauth2_auth.rs** - OAuth2 device code flow implementation with token caching
- **models.rs** - Data structures for API responses (sites, nodes, storage areas, data locations)
- **mount.rs** - Mount/unmount utility for data access