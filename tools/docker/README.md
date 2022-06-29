# zCore develop docker image

Build docker image

```
git clone https://github.com/rcore-os/zCore --recursive
cd tools/docker
OS_TYPE=ubuntu
OS_TAG=20.04
IMAGE_TAG=latest
./build_docker_image.sh ${OS_TYPE} ${OS_TAG} ${IMAGE_TAG}
```

Start docker container

```
IMAGE_NAME=zcore:${OS_TYPE}-${OS_TAG}-${IMAGE_TAG}
./start_container.sh ${IMAGE_NAME}
```
