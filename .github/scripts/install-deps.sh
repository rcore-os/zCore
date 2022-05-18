#!/usr/bin/env bash

sudo apt-get update
sudo apt-get install -y $@
pip3 install -r tests/requirements.txt