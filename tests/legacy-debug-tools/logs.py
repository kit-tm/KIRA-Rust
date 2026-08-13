import ipaddress
import docker
import json
import re
from concurrent.futures import ThreadPoolExecutor, as_completed

DOCKER_CLIENT = docker.from_env(version="auto", max_pool_size=20)

def load_replacements(idmap_file):
    with open(idmap_file, 'r') as f:
        replacements = json.load(f)

    # Invert the dict to map node_id to node_number
    replacements = {v: k for k, v in replacements.items()}

    # Extend the replacements dict with the IPv6 addresses
    for node_id in list(replacements.keys()):
        ip_address = node_id_to_ip(node_id)
        replacements[ip_address] = replacements[node_id]
    return replacements

def node_id_to_ip(node_id):
    ip_parts = [node_id[i:i+4] for i in range(0, len(node_id), 4)]
    ip_address = f"fc00:{':'.join(ip_parts)}"
    return ipaddress.ip_address(ip_address).compressed

def replace_ids(logs, pattern, replacements):
    def replace(match):
        return replacements[match.group(0)]
    return pattern.sub(replace, logs)

def fetch_logs(container_name):
    try:
        container = DOCKER_CLIENT.containers.get(container_name)
        return container.logs().decode('utf-8')
    except Exception as e:
        return f"Error fetching logs from {container_name}: {str(e)}"

def process_logs(container_name, node_map, pattern):
    logs = fetch_logs(container_name)
    processed_logs = replace_ids(logs, pattern, node_map)

    filename = f"{container_name.split('.')[1]}.log"
    with open(filename, 'w') as file:
        file.write(processed_logs)

    return f"Logs from {container_name} saved to {filename}"

def main():
    node_map = load_replacements('idmap.json')
    pattern = re.compile("|".join(re.escape(key) for key in node_map.keys()))
    all_containers = DOCKER_CLIENT.containers.list()
    containers = [container.name for container in all_containers if container.name.startswith("mn.k")]

    with ThreadPoolExecutor(max_workers=20) as executor:
        futures = {executor.submit(process_logs, name, node_map, pattern): name for name in containers}
        for future in as_completed(futures):
            container_name = futures[future]
            try:
                result = future.result()
                print(result)
            except Exception as e:
                print(f"Error processing logs from {container_name}: {str(e)}")

if __name__ == "__main__":
    main()