# Compatibilidad con Linux guiada por `Documentation/`

Eclipse OS implementa su capa Linux (`linux-syscall` + `linux-object`)
tomando como referencia el árbol de documentación del kernel:
<https://github.com/torvalds/linux/tree/master/Documentation> (junto con las
páginas man de man7.org, que documentan la UAPI estable). Esta página mapea
los subsistemas de compatibilidad con los documentos que los guían y registra
las decisiones donde nuestro kernel difiere deliberadamente.

## Identidad de proceso y control de trabajos

| Interfaz | Referencia | Estado |
|---|---|---|
| `getpid`/`getppid`/`gettid`, TID del líder == PID | proc.rst | Completo |
| Grupos de proceso: `setpgid`/`getpgid`/`getpgrp` | credentials.rst, proc.rst | Completo (modelo permisivo, sin validación de sesión cruzada) |
| Sesiones: `setsid`/`getsid` | credentials.rst | Real: sid por proceso, heredado en `fork`; `setsid` falla con `EPERM` para líderes de grupo. No se modela el desapego del terminal controlador (cada proceso está ligado a su VT) |
| `prctl(PR_SET_NAME/PR_GET_NAME)` | prctl(2); `TASK_COMM_LEN` en sched.h | `comm` real por hilo, visible en `/proc/<pid>/comm`; `execve` lo reinicia al basename del binario |
| `prctl(PR_SET_PDEATHSIG)` | prctl(2) | Con entrega real al morir el padre; se limpia en el hijo tras `fork`, como Linux |
| `prctl(PR_SET_CHILD_SUBREAPER)` | prctl(2) | Los huérfanos se reparentan al subreaper vivo más cercano en vez de a init (como usan tmux / gestores de sesión) |
| `prctl(PR_SET_NO_NEW_PRIVS)` | **Documentation/userspace-api/no_new_privs.rst** | Flag unidireccional, heredado por `fork`/`execve`; con él activo `execve` no honra bits setuid/setgid |
| `prctl` dumpable / timerslack / THP / capbset / tid_address | prctl(2) | Estado real por proceso/hilo; opciones desconocidas → `EINVAL` (antes *todas* devolvían 0, afirmando p. ej. que seccomp estaba activo) |
| `personality` | personality(2) | Persona por proceso (PER_LINUX + modificadores como `ADDR_NO_RANDOMIZE`), heredada; consultable con 0xffffffff |

## Señales

| Interfaz | Referencia | Estado |
|---|---|---|
| `rt_sigaction`/`rt_sigprocmask`/`rt_sigreturn`/`sigaltstack` | signal.rst (core-api) | Completo |
| `rt_sigpending` | sigpending(2) | Conjunto pendiente-y-bloqueado del hilo |
| `rt_sigqueueinfo` / `rt_tgsigqueueinfo` | rt_sigqueueinfo(2) | Entrega real con validación de `si_code` (`EPERM` si se intenta falsificar códigos del kernel hacia otro proceso). El conjunto pendiente es un bitmask: el payload `si_value` no se conserva |
| `sigsuspend`/`sigtimedwait`/`signalfd` | signalfd.rst | Completo (siginfo mínimo) |
| pdeathsig al morir el padre | prctl(2) | Ver arriba |

## IPC System V (sysvipc(7), proc.rst → `/proc/sysvipc`)

| Mecanismo | Estado |
|---|---|
| Semáforos (`semget`/`semop`/`semctl`) | Preexistente; ids por proceso, rendez-vous global por clave |
| Memoria compartida (`shmget`/`shmat`/`shmdt`/`shmctl`) | Preexistente |
| **Colas de mensajes** (`msgget`/`msgsnd`/`msgrcv`/`msgctl`) | Nuevo: ids globales, persistencia hasta `IPC_RMID` (sysvipc(7)), envío/recepción bloqueantes e interrumpibles, `MSG_NOERROR`/`MSG_EXCEPT`/`IPC_NOWAIT`, límites `MSGMAX`/`MSGMNB` de Documentation/admin-guide/sysctl/kernel.rst, y tabla en `/proc/sysvipc/msg` para `ipcs -q` |

## Sistema de archivos

| Interfaz | Referencia | Estado |
|---|---|---|
| `renameat2` | renameat2(2), Documentation/filesystems/porting.rst | `RENAME_NOREPLACE` real (falla con `EEXIST`); `RENAME_EXCHANGE`/`WHITEOUT` → `EINVAL`, la respuesta documentada de un fs sin soporte |
| `syncfs` | syncfs(2) | Sincroniza el fs del descriptor |
| `/proc` (proc.rst) | **Documentation/filesystems/proc.rst** | pid: stat (con pgrp/session/tty_nr/tpgid reales), status, maps, fd, comm, environ, statm, exe, cmdline; global: meminfo, stat, uptime, loadavg, mounts, net/*, sys/*, sysvipc/msg |
| xattrs | xattr.rst | Sin soporte en los fs: respuestas estándar (`ENODATA`/`EOPNOTSUPP`), no `ENOSYS` |

## Memoria

| Interfaz | Referencia | Estado |
|---|---|---|
| `mlock`/`mlock2`/`munlock`/`mlockall`/`munlockall` | **Documentation/mm/unevictable-lru.rst**, mlock(2) | Validación de rango/flags como Linux; sin swap las páginas mapeadas son permanentemente residentes, así que el éxito es la respuesta veraz (lo que gpg/ssh-agent necesitan) |
| `mmap`/`mprotect`/`munmap`/`mremap`/`madvise`/`mincore` | Documentation/mm/ | Preexistente (anónimo con demand paging) |
| overcommit / max_map_count en `/proc/sys/vm` | Documentation/admin-guide/sysctl/vm.rst | Valores por defecto de Linux |

## Decisiones deliberadas

- **`clone3` → `ENOSYS`**: comportamiento pre-5.3; glibc/musl caen a `clone`
  limpiamente (ver comentario en `linux-syscall/src/lib.rs` con la causa raíz).
- **`rseq` → `ENOSYS` silencioso**: glibc lo sondea y cae con gracia.
- **seccomp**: `prctl(PR_GET/SET_SECCOMP)` responde `EINVAL` como un kernel
  compilado sin `CONFIG_SECCOMP`; el syscall `seccomp` queda en `ENOSYS`.
- **Colas de mensajes**: `msgrcv` despierta por sondeo corto (5 ms) en vez de
  con una cola de waiters; suficiente para las cargas que las usan, y el
  camino queda interrumpible por señales (`EINTR`) como pide msgsnd(2).

## Pendiente (candidatos siguientes, con su documento guía)

- `execveat`/`fexecve` (execveat(2)).
- `process_vm_readv`/`writev` (cross-memory attach).
- `/proc/sysvipc/{sem,shm}` (proc.rst) para completar la tríada.
- `openat2` con `RESOLVE_*` (Documentation/filesystems/path-lookup.rst).
- Colas POSIX `mq_*` (mq_overview(7)).
