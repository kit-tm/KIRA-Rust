# KIRA base image
This is the base image
It is based on running multiple services using `supervisord`.

## Services
### KIRA Daemon
You can get the local IPv6 with the following 

### DNS-DHT-Resolver
A container-local dht stub resolver resolving ".kira.internal" domains.
It also supports registering by sending DNS Update Queries:
```bash
nsupdate
   server 127.0.0.1
   zone kira.internal
   update add myname.kira.internal 86400 AAAA  
   show
   send

```

### dns-registration

### 4in6-tunnels

## Configuring

## Extending



