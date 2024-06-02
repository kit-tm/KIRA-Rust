# KIRA Example with DHT-DNS and running additional services

Setup with inline explanations is in [example.py](./example.py)

This examples features 3 types of nodes: 
- base (image: kira-example-base)
- server (image: kira-example-base)
    - additionally runs a REST-API with a dice-roll endpoint
- client (image: kira-example-base)
    - additionally runs repeated queries to the REST-API

## Building and running 
### Dependencies
- Installed [Containernet](https://containernet.github.io/#installation)
- Built base kira image: `kira` (`make build-image-supervisord` in repo root)

### Building
```shell
make build-images
```

### Running
```shell
./example.py
```

### Testing
You should now have a containernet CLI. Wait some time (20s) for everything to converge:

curl the REST-API on server (`roll.kira.internal`):
```
d1 curl -6 -v http://roll.kira.internal/ # s1.kira.internal also works
```

This should fail as `d1` has no v4 tunnel setup:
```
d1 curl -4 -v http://roll.kira.internal/ 
```

Client `c1` has though:
```
c1 curl -4 roll.kira.internal # s1.kira.internal also works
```

## Base Image (in [./base])
This uses multiple services using supervisord:
- the kira routing daemon (kirad)
- a local DNS server using the DHT to register and resolve [./base/dns-dht-resolver.py]
- a registration script registering each node under <hostname>.kira.internal [./base/dns-registration.sh]
- a IPv4 tunneling script that sets up 4in6 tunnels between nodes [./base/dns-v4-tunnel-setup.sh]

## Extending the base image
If you want to run additional software inside the containers refer to the server-Dockerfile: [./server/Dockerfile]
