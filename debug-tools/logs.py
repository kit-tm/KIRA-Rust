import docker
import re
import json
import ipaddress
from concurrent.futures import ThreadPoolExecutor

def node_id_to_ip(node_id):
    ip_parts = [node_id[i:i+4] for i in range(0, len(node_id), 4)]
    ip_address = 'fc00:' + ':'.join(ip_parts)
    return ipaddress.ip_address(ip_address).compressed

def replace_ids(logs, node_map):
    for node, node_id in node_map.items():
        node_number = f"{node}"
        logs = re.sub(node_id, node_number, logs)
        ip_address = node_id_to_ip(node_id)
        logs = re.sub(re.escape(ip_address), node_number, logs)
    return logs

def fetch_logs(container_name):
    client = docker.from_env()
    try:
        container = client.containers.get(container_name)
        return container.logs().decode('utf-8')
    except Exception as e:
        return f"Error fetching logs from {container_name}: {str(e)}"

def process_logs(container_name, node_map):
    logs = fetch_logs(container_name)
    processed_logs = replace_ids(logs, node_map)

    filename = f"{container_name.split('.')[1]}.log"
    with open(filename, 'w') as file:
        file.write(processed_logs)

    return f"Logs from {container_name} saved to {filename}"

def main():
    with open('idmap.json', 'r') as file:
        node_map = json.load(file)

    containers = [f"mn.k{i}" for i in range(len(node_map))]

    with ThreadPoolExecutor(max_workers=20) as executor:
        futures = {executor.submit(process_logs, name, node_map): name for name in containers}
        for f in futures:
            try:
                result = f.result()
                print(result)
            except Exception as e:
                print(f"Error processing logs from {futures[f]}: {str(e)}")

if __name__ == "__main__":
    main()
