# Compatibilidad con FreeBSD (ABI FreeBSD/amd64)

Eclipse OS ejecuta binarios de Linux de forma nativa. Este documento describe la
**capa de compatibilidad con FreeBSD/amd64**: la infraestructura que permite que
un ejecutable etiquetado como FreeBSD entre al kernel por la misma instrucción
`syscall`, pero hablando la ABI de FreeBSD (números de syscall, codificación de
flags, disposición de structs y convención de retorno propios).

La capa está inspirada directamente en el árbol de fuentes
[`freebsd/freebsd-src`](https://github.com/freebsd/freebsd-src): cada tabla de
constantes se transcribe de su cabecera de origen y el comentario cita el fichero
correspondiente para que se pueda contrastar con el upstream.

> **Alcance.** Es una **base de compatibilidad**, no un kernel FreeBSD completo.
> Está pensada para que un binario **estático** de FreeBSD progrese por el
> arranque de libc hasta `main` y ejecute E/S de ficheros, memoria, procesos y
> tiempo. Lo que todavía **no** está implementado se enumera en
> [Limitaciones](#limitaciones).

Solo aplica a **x86_64**: la ABI implementada es FreeBSD/amd64. En otras
arquitecturas todo proceso se ejecuta como Linux.

## Cómo funciona

### 1. Detección de la ABI (personalidad)

Al cargar un ELF, el cargador (`linux-object/src/loader/mod.rs`, `detect_abi`)
decide la *personalidad* del proceso:

- **`EI_OSABI == ELFOSABI_FREEBSD` (9)** en `e_ident[7]` — la señal que la
  toolchain de FreeBSD estampa en los ejecutables estáticos.
- Como respaldo, se escanean los segmentos `PT_NOTE` en busca del nombre de
  proveedor `"FreeBSD"` (algunos binarios dejan `EI_OSABI` en `SYSV` y usan la
  nota `NT_FREEBSD_ABI_TAG`).

La personalidad (`linux_object::process::Abi`) se guarda en el `LinuxProcess`, se
**hereda en `fork`** y se **reevalúa en `execve`** (un shell Linux puede
`execve` un binario FreeBSD y viceversa).

### 2. Pila inicial y vector auxiliar

Un binario estático de FreeBSD necesita una pila con forma FreeBSD, distinta de
la de Linux (`linux-object/src/loader/abi.rs`, `push_at_freebsd`):

- Vector auxiliar con los tipos `AT_*` de FreeBSD (`sys/sys/elf_common.h`):
  `AT_PHDR/PHENT/PHNUM/BASE/ENTRY/PAGESZ`, más los específicos de FreeBSD
  `AT_EXECPATH`, `AT_CANARY`/`AT_CANARYLEN` (canario SSP), `AT_PAGESIZES`,
  `AT_OSRELDATE`, `AT_NCPUS`, `AT_STACKPROT`, `AT_PS_STRINGS`,
  `AT_USRSTACKBASE`/`LIM`.
- Un bloque `struct ps_strings` y un canario SSP con bytes aleatorios reales.
- Registros de entrada según `exec_setregs` (`sys/amd64/amd64/exec_machdep.c`):
  `%rdi` = puntero a `argc`, `%rsp` alineado a 8 mod 16.

### 3. Despacho de syscalls y convención de retorno

En el manejador de traps (`loader/src/linux.rs`), si la personalidad es FreeBSD
la syscall se despacha por `Syscall::bsd_syscall` (`linux-syscall/src/bsd/`) en
lugar del camino Linux. La convención de retorno de amd64 se aplica al contexto
de usuario (`cpu_set_syscall_retval`, `sys/amd64/amd64/vm_machdep.c`):

| resultado | `%rax` | `%rdx` | acarreo (CF) |
|-----------|--------|--------|--------------|
| éxito     | valor  | valor secundario | 0 |
| error     | errno positivo | — | **1** |

Los stubs de la libc de FreeBSD ramifican según el acarreo (`jb .cerror`), por lo
que fijar el CF es lo que distingue error de éxito. El `sysret` del kernel
restaura `RFLAGS` desde el marco de trap, así que basta con escribir el bit de
acarreo en `general.rflags`.

`fork` recibe el trato especial de FreeBSD: el hijo devuelve `%rax=0`, `%rdx=1` y
CF despejado (`cpu_fork`).

### 4. Traducciones

La libc de FreeBSD pasa constantes con codificación FreeBSD; los métodos `sys_*`
reutilizados esperan las de Linux. Todo esto se traduce:

| Aspecto | Fichero | Nota |
|---------|---------|------|
| Números de syscall | `bsd/consts.rs` (`sys`) | `sys/sys/syscall.h` |
| `errno` | `bsd/errno.rs` | `sys/sys/errno.h`. **`EAGAIN`↔`EDEADLK` están intercambiados** entre Linux y FreeBSD |
| Flags de `open`/`*at` | `bsd/translate.rs` | `sys/sys/fcntl.h` (p. ej. `O_CREAT` 0x0200 vs 0x40, `O_CLOEXEC` 0x100000 vs 0x80000) |
| Flags de `mmap` | `bsd/translate.rs` | `sys/sys/mman.h` (`MAP_ANON` 0x1000 vs 0x20) |
| `struct stat` (224 B) | `bsd/fs.rs` | `sys/sys/stat.h` |
| `getdirentries` / `dirent` | `bsd/fs.rs` | `sys/sys/dirent.h` |
| `clockid_t` | `bsd/mod.rs` | `sys/sys/_clock_id.h` (`MONOTONIC` 4 vs 1) |
| `sysctl`/`sysctlbyname` | `bsd/sysctl.rs` | `sys/kern/kern_mib.c` |
| `sysarch` (FS base / TLS) | `bsd/mod.rs` | `sys/amd64/amd64/sys_machdep.c` |

### Syscalls FreeBSD propias soportadas

- `__sysctl` / `__sysctlbyname` para las hojas `kern.*` y `hw.*` que la libc
  consulta al arrancar (`kern.ostype`, `kern.osreldate`, `hw.pagesize`,
  `hw.ncpu`, `kern.usrstack`, `kern.arandom`, …).
- `sysarch(AMD64_SET_FSBASE/GET_FSBASE)` — base de TLS.
- `thr_self` — id del hilo.
- `issetugid`, `getpid` (con `%rdx`), `__getcwd`, `getdirentries`.

## Cómo probarlo

No se puede ejecutar de extremo a extremo sin arrancar el kernel con un binario
FreeBSD en el rootfs. Un programa estático mínimo (freestanding, sin libc) sirve
para validar la ruta completa: detección → pila → despacho → convención CF.

```asm
# freebsd-hello.s — FreeBSD/amd64, estático, sin libc.
# Ensamblar en un host FreeBSD (o con un cross-as) y marcar la nota de ABI:
#   cc -static -nostdlib -o hello freebsd-hello.s
#   brandelf -t FreeBSD hello      # fija EI_OSABI=9 por si el enlazador no lo hizo
.text
.global _start
_start:
    # write(1, msg, len)
    movq  $4, %rax          # SYS_write
    movq  $1, %rdi          # fd = stdout
    leaq  msg(%rip), %rsi
    movq  $14, %rdx         # len
    syscall
    jc    fail              # CF=1 => error (convención FreeBSD)

    # exit(0)
    xorq  %rdi, %rdi
    movq  $1, %rax          # SYS_exit
    syscall
fail:
    movq  $1, %rdi
    movq  $1, %rax          # SYS_exit(1)
    syscall
.data
msg:
    .ascii "hola FreeBSD\n\0"
```

Copie el ejecutable resultante al rootfs y láncelo con `ROOTPROC` (ver el
[README principal](../README.md)). En el log del kernel debería aparecer:

```
elf: detected FreeBSD ABI for "/bin/hello"
```

Los mapeos puros (números, `errno`, flags, tamaño de `struct stat`, `sysctl`,
`clockid`) están cubiertos por tests unitarios:

```bash
cargo test -p linux-syscall --lib bsd::
```

## Limitaciones

Estas piezas todavía **no** están implementadas y son el trabajo natural de
seguimiento para ejecutar binarios FreeBSD reales:

- **Señales.** `sigaction`/`sigprocmask` se aceptan como no-op (devuelven 0) pero
  la entrega usa la disposición por defecto: falta el marco de señal
  (`sigframe`) y `sigreturn` de FreeBSD, cuya disposición y struct difieren de
  Linux.
- **Hilos.** La ABI `thr_new`/`thr_create`/`thr_exit` no está implementada
  (`ENOSYS`); solo `thr_self` responde. `_umtx_op` es un no-op que solo es
  correcto para el caso monohilo sin contención.
- **Enlazador dinámico.** Solo se soportan binarios **estáticos**. Un ejecutable
  dinámico necesita `/libexec/ld-elf.so.1`, que este árbol no incluye.
- **`ioctl`.** Los números de `ioctl` de FreeBSD (p. ej. `TIOCGETA`) difieren de
  los de Linux (`TCGETS`) y todavía no se traducen; `isatty` y el control de
  terminal pueden fallar.
- **`sysctl`.** Solo se modela un subconjunto de hojas `kern.*`/`hw.*`. Las no
  modeladas devuelven `ENOENT`.
- La forma exacta de la pila (`ps_strings`, alineación de `%rsp`) está construida
  siguiendo `exec_copyout_strings`/`exec_setregs` pero aún no se ha validado
  contra un binario FreeBSD real ejecutándose.

## Mapa de ficheros

| Fichero | Contenido |
|---------|-----------|
| `linux-syscall/src/bsd/consts.rs` | Constantes de la ABI transcritas de `freebsd-src` |
| `linux-syscall/src/bsd/errno.rs` | Traducción de `errno` Linux → FreeBSD |
| `linux-syscall/src/bsd/translate.rs` | Traducción de flags `open`/`*at`/`mmap` |
| `linux-syscall/src/bsd/fs.rs` | `struct stat` (224 B) y `dirent` de FreeBSD |
| `linux-syscall/src/bsd/sysctl.rs` | `sysctl`/`sysctlbyname` |
| `linux-syscall/src/bsd/mod.rs` | Despacho + convención de retorno CF/`%rdx` |
| `linux-object/src/loader/mod.rs` | Detección de la ABI en el ELF |
| `linux-object/src/loader/abi.rs` | Construcción de la pila/auxv de FreeBSD |
| `linux-object/src/process.rs` | Personalidad `Abi` por proceso |
| `loader/src/linux.rs` | Despacho por personalidad en el manejador de traps |
