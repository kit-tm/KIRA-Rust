#!/usr/bin/env python3
import requests
from fastapi import FastAPI
import random

app = FastAPI()

@app.get("/")
def get_dice_roll():
    return random.randint(1,6)
