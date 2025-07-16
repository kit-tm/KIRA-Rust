[Unit]
Description=r2kad — A KIRA routing daemon written in Rust
After=network.target
Before=network-online.target

[Service]
Environment=RUST_LOG="debug"
ExecStart=BIN_DIR/kirad -n SHARE_DIR/nftables.conf -e "%i"
ExecReload=/bin/kill -HUP $MAINPID
Restart=on-failure
ProtectSystem=full

[Install]
WantedBy=multi-user.target
