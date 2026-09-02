# Path Finder

**pathFinder** is a tool for mounting SKA data on Slurm clusters without copying the data locally. Currently, it is provided as a single binary or an RPM installer.

The tool allows the Scientist to specify which files, identified from the Science Gateway, they want to mount while keeping the files secure and owned by them. Two methods are planned: interactive and a workflow managed by the Science Gateway via prepareData.

Features:

- Bearer-token authentication against the SKA-IAM JWKS, with token exchange for SRCNet API tokens
- Data location lookup via Data Management API
- Site capabilities verification via Site Capabilities API
- Secure data mounting with proper permissions

## Installation

For instructions on the requirements and setup for your HPC server environment, and installation of **pathFinder** itself, see the [SERVER-CONFIGURATION.md](./SERVER-CONFIGURATION.md) doc.

## Usage

### Mount Data

The `pathFinder` command is available to run on the CLI after logging into the Slurm login node. The command needs to be run as `sudo` because it is mounting data, users in the `pathfinder` group are granted `sudo` privileges to execute the `pathFinder` executable.

    $ sudo pathFinder --help

    A CLI tool for mounting SKA data.

    Usage: pathFinder [OPTIONS] --namespace <NAMESPACE> --file-name <FILE_NAME>

    Options:
          --namespace <NAMESPACE>  Namespace of the data (e.g. "teal")
          --file-name <FILE_NAME>  Name of the data file within the namespace
          --token [<TOKEN>]        Specify a JWT token instead of interactive OAuth flow; falls back to PATHFINDER_SKA_AUTH_TOKEN if TOKEN value is omitted
          --unmount                Unmount previously mounted data instead of mounting
      -h, --help                   Print help

#### Authentication

`pathFinder` needs a valid bearer token issued by the SKA Identity and Access
Management (IAM) service. The token is validated against the IAM JWKS endpoint
and then exchanged for Data Management and Site Capabilities API tokens
internally. It can be provided in either of the following ways.

##### Command-line token

    $ sudo pathFinder --namespace daac --file-name simple_file.txt --token "eyJhbG..."

    Validating bearer token against the JWKS...
    Authenticated as subject 'alice@example.org'
    RSE Path for file 'simple_file.txt' in namespace 'daac': /daac/14/66/simple_file.txt
    Mount verification successful: simple_file.txt is mounted at /home/sm2921/projects/daac/simple_file.txt

##### Environment variable

For automation, the token can be set in the `PATHFINDER_SKA_AUTH_TOKEN`
environment variable instead of passing `--token`. If `--token` is supplied
without a value, the environment variable is used automatically.

    $ export PATHFINDER_SKA_AUTH_TOKEN="eyJhbG..."
    $ sudo pathFinder --namespace daac --file-name simple_file.txt

### Unmount Data

Example for unmounting a file:

    $ sudo pathFinder --namespace daac --file-name simple_file.txt --unmount
    Unmounted simple_file.txt from /home/sm2921/projects/daac/simple_file.txt

## HTTP server daemon

The `pathfinder-http` binary runs the pathFinder HTTP API as a `systemd` service.
It is packaged as an RPM that installs the binary, a systemd unit, and a default
environment file.

### Install the RPM

    VERSION=2.0.0
    sudo dnf install https://github.com/uksrc/pathFinder/releases/download/v${VERSION}/pathfinder-http-${VERSION}-1.x86_64.rpm

The install enables and starts the `pathfinder-http` service automatically.

### Configure the daemon

Edit `/etc/default/pathfinder-http` to change the database path or the listen
address:

    PATHFINDER_HTTP_DB_PATH=/var/lib/pathfinder-http/pathfinder.db
    PATHFINDER_HTTP_LISTEN_ADDR=0.0.0.0:8765

Then restart the service:

    sudo systemctl restart pathfinder-http

### Service endpoints

- `POST /stage-in` — start an asynchronous stage-in request.
- `GET /stage-in/{request_id}` — poll the status of a stage-in request.
- `POST /stage-out` — trigger stage-out for a completed request.

## Development

Notes on how to build the executable, run the unit and integration tests, and local Docker-based testing can be found in the development can be found in the [DEVELOPMENT.md](DEVELOPMENT.md) doc.
