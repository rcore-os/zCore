# Banco de pruebas del driver e1000e (apk update bajo QEMU)

Reproduce, de forma autocontenida y sin acceso a Internet, el escenario que
estresa el driver `drivers/src/net/e1000e.rs`: la descarga del índice de
paquetes por `apk update` y transferencias TCP de varios MB.

En lugar del mirror real de Alpine (`dl-cdn.alpinelinux.org`, normalmente
inalcanzable en CI/entornos restringidos) se levanta un **mirror local firmado**
servido por HTTP en el host. QEMU-SLIRP enruta `10.0.2.2` → host, así que el
guest ejecuta `apk update` contra ese mirror ejercitando el camino RX/TX del
driver. Si el driver pierde o corrompe paquetes, `apk` reporta `BAD signature` /
`invalid or inconsistent`; si es correcto, da 0 fallos.

## Uso

```bash
# 1. Rootfs + imagen + ESP (una vez; requiere QEMU, mtools, dosfstools)
cargo rootfs --arch x86_64
cp tools/e1000e-bench/bench.sh rootfs/x86_64/bench.sh   # init del benchmark
cargo image --arch x86_64
make -C zCore build MODE=release LINUX=1 LOG=warn GRAPHIC=off \
  CMDLINE='LOG=warn:console.shell=true:virtcon.disable=true:ROOT=/dev/vda:ROOTPROC=/bin/busybox?sh?/bench.sh'

# 2. Generar el mirror local firmado (índice grande + bigfile)
NUM_RECORDS=40000 BIGFILE_MB=16 bash tools/e1000e-bench/build-mirror.sh

# 3. Lanzar QEMU + mirror y evaluar (timeout en segundos)
bash tools/e1000e-bench/run-bench.sh 720
```

El guest fija `/etc/apk/repositories` a `http://10.0.2.2:8080/repo` y confía en
la clave del mirror vía `/etc/apk/keys` (la pública se copia a
`prebuilt/alpine-apk-keys/` durante `build-mirror.sh` y `cargo rootfs` la mete
en el rootfs).

## Marcadores de salida (serie)

- `BENCH_WGET i OK|FAIL`     integridad por wget+sha256 de `bigfile.bin`
- `BENCH_WGET_FAILS=n/N`
- `BENCH_APK i OK|CORRUPT|NOIDX`  `apk update --no-cache`
- `BENCH_APK_CORRUPT=n/N`    corridas con firma mala / índice inconsistente (corrupción del driver)
- `BENCH_APK_NOIDX=n/N`      corridas sin índice (fallo de conexión)

Objetivo: `BENCH_APK_CORRUPT=0/N` y `BENCH_WGET_FAILS=0/N`.

## Notas

- Sin `/dev/kvm` se usa TCG (emulación), más lento pero válido para integridad.
- La clave privada y el repo generado viven bajo `local-mirror/` (gitignored).
- En QEMU, smoltcp verifica los checksums TCP/IP en software, por lo que la
  corrupción se detecta y retransmite; los fallos de corrupción silenciosa
  suelen ser específicos de hardware real (I219-V / coherencia DMA / PCH).
