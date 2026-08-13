# KIRA Routing Daemon Implementation

Rust crate representing the binary implementation of the KIRA routing daemon.
For overall instructions please see the repo's [README](../README.md).

## Test Framework

The test framework is custom-built to be able to run the test cases as integration tests instead of manual ones.
The integration tests themselves are located in the folder [`tests`](./tests).
The test framework and its structures are located in the module [`common`](./tests/common).
