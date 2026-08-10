# Jaeger Tracing

This folder continues a simple configuration and a [Docker Compose](https://docs.docker.com/compose/) file
to get you started with tracing the KIRA routing daemon using [Jaeger](https://www.jaegertracing.io/) in no time.

Additionally we provide a [Greasmonkey](https://github.com/greasemonkey/greasemonkey)
script that allows you to easily substitute every NodeId with it's corresponding
TopologyId in the displayed Jeager traces.

## Running

If you want to use [`nesttest.py`](../nesttest.py) you can invoke the helper
script in a similar fashion as you'd invoke [`nesttest.py`](../nesttest.py):

```sh
tests/jaeger/jaeger-tracing.sh tests/topos/minimal.gml
```

This will build the Docker and [NeST](https://nest.nitk.ac.in/) topologies
using Docker Compose and NeSt respectively.

### Building kirad

For this to work you have to ensure that kirad is build with [OpenTelemetry](https://opentelemetry.io/) support.
You have to enable the `otel` feature flag on _build time_:

```sh
cargo build --features=otel,small_buckets
```


## Greasemonkey Extension ##

To install the Greasmonkey extension you have to firstly [install Greasmonkey](https://addons.mozilla.org/en-US/firefox/addon/greasemonkey/)
from the Firefox add-on store. Alternatively you could use [Tampermonkey](https://www.tampermonkey.net/).

Afterwards simply install the [`node-to-topo.user.js`](./node-to-topo.user.js) using
the provided add-on menus.

You should now be greeted with a blue cog on the bottom right corner where
you can input your custom substitution mapping. This can be obtained
if you're using [`nesttest.py`](../nesttest.py) by simply copying the command
output of

```sh
ntest> nodes
```
