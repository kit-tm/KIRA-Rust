[Unit]
Description=KIRA Routing Daemon
Documentation=https://gitlab.kit.edu/kit/tm/telematics/kira/kira-rust
After=network.target
Before=network-online.target

[Service]
Environment="RUST_LOG=info"
ExecStart=BIN_DIR/kirad -n SHARE_DIR/nftables.conf
ExecStopPost=ip link delete kira
Restart=on-failure

ProtectSystem=full
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
RestrictAddressFamilies=AF_INET AF_INET6 AF_NETLINK
CapabilityBoundingSet=CAP_NET_ADMIN
NoNewPrivileges=yes


[Install]
WantedBy=multi-user.target
