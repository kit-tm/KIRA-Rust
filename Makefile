.PHONY: doc-lib

doc-lib:
	cargo +nightly doc --features unstable-doc-cfg --package r2kad-lib --open
