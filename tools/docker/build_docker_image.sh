#!/bin/bash
set -ex

OS_TYPE=ubuntu
if [ -n "$1" ] && [ "$1" != "${OS_TYPE}" ]; then
    OS_TYPE=$1
fi
DOCKER_FILE=zcore-${OS_TYPE}.dockerfile

OS_TAG=20.04
if [ -n "$2" ] && [ "$2" != "${OS_TAG}" ]; then
    OS_TAG=$2
fi
BASE_IMAGE=${OS_TYPE}:${OS_TAG}

IMAGE_TAG=latest
if [ -n "$3" ] && [ "$3" != "${IMAGE_TAG}" ]; then
    IMAGE_TAG=$3
fi
IMAGE_NAME=zcore:${OS_TYPE}-${OS_TAG}-${IMAGE_TAG}

http_proxy=""
https_proxy="${http_proxy}"
no_proxy="localhost,127.0.0.1"
DOCKER_BUILDKIT=0

docker build \
    -f ${DOCKER_FILE} \
    -t ${IMAGE_NAME} \
    --network=host \
    --build-arg no_proxy=${no_proxy} \
    --build-arg http_proxy=${http_proxy} \
    --build-arg https_proxy=${https_proxy} \
    --build-arg BASE_IMAGE=${BASE_IMAGE} \
    ../..
