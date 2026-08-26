# Tests de multihilo del kernel (clone/futex)

Tests *freestanding* (sin libc) que reproducen la interacción de
`pthread_create` de musl con el kernel y validan las primitivas en las que se
apoya todo programa multihilo (sysbench, etc.):

- **thr.c** — creación de hilos: `clone` con los flags exactos de musl 1.2
  (`0x7d0f00`), TLS por `CLONE_SETTLS`, `CLONE_PARENT_SETTID`, semántica de
  `ctid` (el kernel **no** debe escribir el TID al crear si no hay
  `CLONE_CHILD_SETTID`, y debe limpiar+despertar en la salida por
  `CLONE_CHILD_CLEARTID` — de esto depende `pthread_join`).
- **thr2.c** — estrés de `FUTEX_WAIT`/`FUTEX_WAKE` entre dos hilos (20k
  iteraciones de ping-pong). Detecta *lost wakeups*: si el kernel comprueba el
  valor del futex fuera del lock de la cola de espera, un WAKE puede colarse
  entre la comprobación y el encolado y el hilo duerme para siempre (así se
  colgaba sysbench en «Initializing worker threads...»).

Ambos hacen las syscalls a mano: con la instrucción `syscall` en bare metal y
a través del puntero `rcore_syscall_entry` (parcheado por el loader) en libos.

Véase el `Makefile` para compilar y ejecutar.

## Verificación con sysbench real

`build-sysbench.sh` compila sysbench 1.0.20 (dinámico, musl, con LuaJIT
empaquetado) — el mismo programa que se colgaba en "Initializing worker
threads...". Con el kernel corregido llega a "Threads started!" y completa el
benchmark de CPU. Verificado en QEMU (`-smp 4`) con 1 y con 4 hilos:

    sysbench cpu --time=10 run            -> ~240 ev/s, termina
    sysbench cpu --time=10 --threads=4 run -> ~960 ev/s (escala ~4x), termina

Si en tu sistema sysbench sigue colgándose pero estos tests pasan, casi seguro
estás arrancando un kernel anterior a los fixes (reinstala `zcore.elf` en el
ESP / regenera la ISO antes de probar).
