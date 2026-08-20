
build-images: build-image-dns build-image-client build-image-server

build-image-dns:
	docker build -t kira-example-base ./base

build-image-client: build-image-dns
	docker build -t kira-example-client ./client

build-image-server: build-image-dns
	docker build -t kira-example-server ./server

