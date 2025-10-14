sudo rm -f /run/firecracker.socket

#tap dev clean up
TAP_DEV="tap0"

sudo ip link delete "$TAP_DEV"

sudo ip tuntap add dev tap0 mode tap user $USER
sudo ip link set tap0 up
sudo ip addr add 192.168.100.1/24 dev tap0

sudo iptables -t nat -A POSTROUTING -o eth0 -j MASQUERADE
sudo iptables -A FORWARD -i tap0 -j ACCEPT
sudo iptables -A FORWARD -o tap0 -m state --state ESTABLISHED,RELATED -j ACCEPT

echo 1 | sudo tee /proc/sys/net/ipv4/ip_forward

sudo ./build/cargo_target/x86_64-unknown-linux-musl/debug/firecracker --config-file config_balloon --log-path logs.fifo --level Info
