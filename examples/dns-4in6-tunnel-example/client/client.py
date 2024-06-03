#!/usr/bin/env python3
import requests
import time

URL="http://roll.kira.internal/"

while True:
    try:
        r = requests.get(URL)
        print(f"Rolled a dice using {URL} and got: {r.text}")
    except:
        print("Error during HTTP request")

    time.sleep(5)
