ARG BASE_IMAGE=ubuntu:20.04
FROM ${BASE_IMAGE}

ENV DEBIAN_FRONTEND=noninteractive
ENV INSTALL_PREFIX=/opt/zcore

RUN apt-get update \
    && apt-get install -y \
        build-essential \
        pkg-config \
        python3 \
        python3-pip \
        meson \
        libglib2.0-dev \
        libpixman-1-dev \
        xz-utils \
        wget \
        curl \
        vim \
    && apt-get clean all \
    && rm -rf /var/lib/apt/lists/* \
    && rm -rf ~/.cache/pip/* \
    && rm -rf /tmp/*

# Install git lfs
RUN curl -s https://packagecloud.io/install/repositories/github/git-lfs/script.deb.sh | bash \
    && apt-get install -y git-lfs \
    && git lfs install

# Install QEMU
RUN mkdir -p ${INSTALL_PREFIX} \
    && wget https://download.qemu.org/qemu-7.0.0.tar.xz \
    && tar -xvJf qemu-7.0.0.tar.xz -C ${INSTALL_PREFIX} \
    && rm -rf qemu-7.0.0.tar.xz \
    && ln -s ${INSTALL_PREFIX}/qemu-7.0.0 ${INSTALL_PREFIX}/qemu \
    && cd ${INSTALL_PREFIX}/qemu \
    && ./configure --target-list=x86_64-softmmu,x86_64-linux-user,riscv64-softmmu,riscv64-linux-user,aarch64-softmmu,aarch64-linux-user \
    && make -j `nproc` \
    && make install \
    && rm -rf ${INSTALL_PREFIX}/qemu/*

# Install rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y

ENV PATH="$HOME/.cargo/bin:$PATH"
ENV WORK_SPACE_PATH=${INSTALL_PREFIX}/zcore

WORKDIR ${WORK_SPACE_PATH}
COPY . .
