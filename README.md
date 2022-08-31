# R²/Kad Implementierung

Dieses Repository sammelt die Informationen zu unterschiedlichen Teilen der Implementierung von R²/Kad in Rust.

- [R²/Kad Routing Daemon](daemon): Enthält die Routing Daemon Anwendung
- [R²/Kad Library](lib): Enthält die verschiedenen abstrakten Komponenten und einige konkrete Implementierungen dieser.

Weitere Informationen sind in den jeweiligen Repositories zu finden.

## Cloning the repository

TO simply clone the repository use this:

```shell
git clone --recurse-submodules git@git.scc.kit.edu:ubesd/r2kad.git
```

If you want to work on the project and its submodules change the branch in the submodules before changing anything.
Otherwise, some weird version control errors will happen.
E.g.

```shell
cd daemon
git checkout -b main
cd ../lib
git checkout -b main
```

## Running

Currently, it's only possible to start the daemon through docker compose in a two node network.
Here are the steps to take for that:

1. Install the required dependencies: [docker](https://docs.docker.com/get-docker/)
   , [docker compose plugin](https://docs.docker.com/compose/install/compose-plugin/),
2. Configure the docker daemon to enable ipv6: [like instructed here](https://docs.docker.com/config/daemon/ipv6/)
3. Create the docker networks:
   ```shell
   docker network create --ipv6 \  
    --subnet="2001:db8:1::/64" \
    --gateway="2001:db8:1::1" \
    mynetv6-1
   ```
   ```shell
   docker network create --ipv6 \                                            
    --subnet="2001:db8:2::/64" \
    --gateway="2001:db8:2::1" \
    mynetv6-2
    ```
4. Build the image:
   ```shell
   docker build -t r2kad-daemon:scratch -f daemon/docker/Dockerfile.scratch .
   ```
   ( NOTE: While the scratch image is very small (~3.12MB) and simply works it doesn't support debugging as nothing but
   the application is in that image.
   To debug the container by running it with a shell you may use
   the [Dockerfile.debian](daemon/docker/Dockerfile.debian) instead [docker-compose.yml](docker-compose.yml) )
5. Start the nodes:
   ```shell
   docker compose up
   ```
6. To stop the nodes press `CTRL+C` and to remove the containers completely use `docker compose down`.
