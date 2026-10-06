Run the original Ubuntu client with the TRUEOS graphics baseline:

```sh
./run-bringup-client.sh
```

This also updates `target/debug/veloren-voxygen`, so launching that binary directly
uses the same baseline. Each startup overlays `trueos-bringup-profile.json` after
loading saved settings. Graphics remain adjustable during the session; the next
launch resets the listed values. Credentials, controls, window settings and other
unlisted values are preserved by the profile.

The JSON uses the original client's settings schema and initially matches
`/home/t4ce/Repos/voxy/trueos-bringup-profile.json`. Edit the local copy and rerun the
script to replay another scenario. Cargo rebuilds the embedded profile as needed.

This is the original Ubuntu scene renderer, not the TRUEOS graphics backend.
