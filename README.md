# Path Finder

A CLI Program to authorise a users access to some srcNet data and return the RSE path.

## TODO Development

- [ ] Always check site capabilities to ensure that the data is staged to your local RSE.
  - [ ] Work out whether we need to check for tier 0.
- [ ] Tidy up the code around checking the response from the DM API `data/locate` request.
- [ ] Use this script to perform the data mount.
- [ ] Investigate whether the data can be specified using the IVO URI.

## HOW TO Try this script during development

1. Ensure you have installed `uv` - <https://docs.astral.sh/uv/>

        uv --version

    NB., you can use other dependency managers which use the `pyproject.toml` - e.g. `poetry`. Hint: `uv` is way faster!

2. Set your Data Management API Access Token:

    1. Navigate to <https://gateway.srcnet.skao.int>
    2. Click your initials badge in the top-right and select "View Token"
    3. Copy the "Data management access token" string
    4. Set the DATA_MANAGEMENT_ACCESS_TOKEN environment variable in your shell:

            export DATA_MANAGEMENT_ACCESS_TOKEN=[PASTED STRING]

3. Run the script while `uv` takes care of the dependencies for you:

        uv run path_finder/path_finder.py

## USE CASE

- A user can invoke this program via the CLI to request the RSE path for a given srcNet data object.
- The user could be authenticated by using OAuth or providing a token.
- The data object could be provided as an IVO URI, or as namespace & filename.
- The script will check that the user is authorised to read the data object.
- If the user is authorised, then the RSE location of the data is obtained.

### Extra use case info

The RSE location will be used to run a `bindfs` command on the parent folder to mount this into the user's `~/.skadata/` directory, setting the user and group to the current user. The specific file from the parent folder will then be used to `mount --bind` that file to `~/skadata/[FILE_NAME]`.
