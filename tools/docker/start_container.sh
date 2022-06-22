set -e

if  [ ! -n "$1" ] ; then
    image=zcore:ubuntu-20.04-latest
else
    image=$1
fi

export http_proxy=""
export http_proxy=${http_proxy}
export no_proxy="localhost,127.0.0.1"

docker run -itd \
    --restart=unless-stopped \
    --privileged=true \
    --net=host \
    --ipc=host \
    -e http_proxy=${http_proxy} \
    -e https_proxy=${https_proxy} \
    -e no_proxy=${no_proxy} \
    -v /home:/home/host-home:rw \
    ${image}
