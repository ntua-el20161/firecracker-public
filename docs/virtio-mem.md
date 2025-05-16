### Boot with virtio-mem
For now there is no API endpoint for virtio-mem. Use the config file option with
the config_vm template (the template exists in repo root) as follows:
```
sudo ./firecracker --config-file config_vm
```
```
socket_location=...

curl --unix-socket $socket_location -i \
    -X GET 'http://localhost/memory-device' \
    -H 'Accept: application/json'
```

```
socket_location=...
requested_size=...

curl --unix-socket $socket_location -i \
    -X PATCH 'http://localhost/memory-device' \
    -H 'Accept: application/json' \
    -H 'Content-Type: application/json' \
    -d "{ \"requested_size_kib\": $requested_size }"
```