# Wireshark Dissectors

This directory contains the dissector of the R²/KAD protocol messages.
Only the CBOR format can be dissected at this point of time.

## Installation

1. Copy the dissector LUA file to `~/.local/lib/wireshark/plugins`.
   If the directory doesn't exist create it.
2. Restart Wireshark.

## Usage

Open your captured traffic file. The R²/KAD protocol should be detected automatically
if the default port wasn't changed. Otherwise you can manually decode the UDP paylod
with the `R2KAD` protocol.
