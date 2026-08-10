# uAPI compatible con nouveau en la GPU NVIDIA — estado

Este documento mapea, ioctl por ioctl, la superficie de `nouveau.ko`
(`include/uapi/drm/nouveau_drm.h` del kernel de Linux) contra lo que
implementa `NvidiaGpu::ioctl` en Eclipse OS. Es el mismo ejercicio que
[`README-drm.md`](README-drm.md) ya hace para la UAPI genérica de DRM/KMS,
aplicado a la superficie *driver-specific* de nouveau.

## Por qué existe esto

Mesa's OpenGL/Vulkan acelerado para NVIDIA (`nouveau_dri.so`, NVK) es
espacio de usuario Linux normal, ya compilado para x86_64 por Alpine, y ya
corre sin modificar bajo la capa de syscalls de Eclipse — igual que Xorg,
labwc o busybox. Lo que le falta es que el **lado de kernel** hable el
mismo protocolo de ioctls que `nouveau.ko`: un contrato público y estable,
a diferencia del protocolo privado y sin documentar entre `nvidia.ko` y el
driver cerrado de NVIDIA. Ver la discusión completa en el hilo que originó
este trabajo.

## Alcance de este hito — leer antes de extender

Esto se escribió en una sandbox **sin GPU NVIDIA, sin `/dev/kvm` y sin QEMU
instalado**. Nada de lo de abajo se ha ejecutado contra hardware real.
Para que eso siga siendo honesto, cada operación es una de estas tres
categorías:

- **(a) Reusa un punto de entrada ya ejercitado en hardware real, tal
  cual** — `nvidia_rm_sys::rm_init::step16`/`step17`, las mismas llamadas
  que hacen `/proc/gpustep16`/`gpustep17`.
- **(b) Es contabilidad pura, sin acceso a hardware/registros** — la tabla
  de handles GEM, el allocador de bitmap de VRAM (`NvidiaVramAllocator`,
  que ya existía como código muerto — nunca se llamaba).
- **(c) Se rechaza explícitamente con `EOPNOTSUPP`/`ENOSYS` y una línea de
  log — nunca se simula un éxito falso.**

**Cómo activarlo**: opt-in estricto, igual que `drm.atomic`. Arranca con
`nvidia.nouveau_uapi` en la `cmdline` (`zCore/rboot.conf`):

```ini
cmdline=LOG=warn:nvidia.nouveau_uapi:ROOTPROC=/bin/busybox?sh
```

Sin el flag, `NvidiaGpu::ioctl` se comporta exactamente igual que antes
(`ENOSYS` para cualquier ioctl no reconocido) — cero cambio de
comportamiento por defecto.

## Cobertura de ioctls

Leyenda: ✅ implementado (real, sin hardware nuevo sin probar) · 🟡 parcial ·
❌ rechazado explícitamente (`EOPNOTSUPP`/`ENOSYS`, nunca simulado).

| ioctl | Estado | Notas |
|---|---|---|
| `DRM_IOCTL_NOUVEAU_GETPARAM` | ✅ | `PCI_VENDOR`/`PCI_DEVICE`/`FB_SIZE`/`VRAM_BAR_SIZE` reales; `CHIPSET_ID` es el mínimo del rango `PMC_BOOT0` de la arquitectura ya identificada por PCI ID (aproximado — no relee `PMC_BOOT0` en vivo, ver más abajo); `VRAM_USED` siempre 0 (el allocador no lleva contador) |
| `DRM_IOCTL_NOUVEAU_SETPARAM` | ❌ | deprecated incluso en Linux; no implementado |
| `DRM_IOCTL_NOUVEAU_CHANNEL_ALLOC` | 🟡 | Reusa la escalera `step16`+`step17` ya existente. **Un solo canal en todo el sistema** — un segundo `CHANNEL_ALLOC` sin `CHANNEL_FREE` antes devuelve `EBUSY`. Los campos legacy (`fb_ctxdma_handle`, `subchan[]`) se ignoran: no aplican a Turing+ |
| `DRM_IOCTL_NOUVEAU_CHANNEL_FREE` | 🟡 | Limpia solo la contabilidad de Eclipse — `nvidia-rm-sys` no tiene un punto de desmontaje real para la escalera `step16`/`step17` (su propio doc la llama "idempotente", pensada para construirse una vez por arranque). Un `CHANNEL_ALLOC` posterior reutiliza la misma asignación cacheada, no crea una nueva |
| `DRM_IOCTL_NOUVEAU_NVIF` | ❌ | no implementado |
| `DRM_IOCTL_NOUVEAU_SVM_INIT` / `SVM_BIND` | ❌ | memoria unificada CPU/GPU — fuera de alcance de este hito |
| `DRM_IOCTL_NOUVEAU_VM_INIT` | 🟡 | Exige un canal ya asignado (`CHANNEL_ALLOC` primero, igual que en Linux real). Devuelve un rango `kernel_managed` vacío (0/0) — placeholder honesto: nada reserva ese rango todavía |
| `DRM_IOCTL_NOUVEAU_VM_BIND` | 🟡 | **Real**: `eclipse_rm_vm_bind_map`/`unmap` (nuevo en `eclipse_rm_init.c`) generalizan el patrón de `step17` (reservar VA en `hVas` + `Map`) para un handle GEM y dirección elegidos por el caller. Limitado a **`op_count == 1`** (más de una operación por llamada: `EOPNOTSUPP`) y **`wait_count == sig_count == 0`** (esperar/señalar aquí no tendría sentido — no hay trabajo de GPU que sincronizar, solo (des)mapeo de VA) |
| `DRM_IOCTL_NOUVEAU_EXEC` | 🟡 | **Real**: `eclipse_rm_exec_submit` generaliza la mecánica de `step18` (GP entry + `GPPut` + timbre) para un *pushbuffer* `(va, len)` que el caller ya escribió. `sig_count == 0` (fire-and-forget) o **`sig_count == 1`** con un DRM syncobj real (ver sección de syncobjs) — `eclipse_rm_exec_submit_signaled` añade una segunda entrada GP con un semáforo propio del kernel y solo marca el syncobj tras confirmar que aterrizó. **`wait_count == 1`** también real, pero por **espera de CPU antes de someter** (`crate::scheme::syncobj::wait`, con timeout fijo de 1 s), NO por un `ACQUIRE` de semáforo ejecutado por el propio canal de hardware — ver la nota en "Huecos conocidos". Limitado a **`push_count == 1`**, `wait_count <= 1` y `sig_count <= 1` |
| `DRM_IOCTL_NOUVEAU_GET_ZCULL_INFO` | ❌ | no implementado |
| `DRM_IOCTL_NOUVEAU_GEM_NEW` | 🟡 | Solo `NOUVEAU_GEM_DOMAIN_VRAM` (memoria de sistema/GART: `EOPNOTSUPP`). **Reserva real vía el heap del RM** (`eclipse_rm_gem_alloc_vram`, clase `NV01_MEMORY_LOCAL_USER` — la misma que usa `step17` para USERD), no un allocador Rust paralelo que podría chocar con la contabilidad propia de RM sobre la misma VRAM. `offset` (VA de GPU) es 0 hasta que `VM_BIND` lo mapea. **`map_handle` real**: `eclipse_rm_gem_map_cpu` (nuevo en `eclipse_rm_init.c`) resuelve el `hMemory` recién asignado a su offset BAR1-relativo real (`memGetByHandle` + `memdescGetPhysAddr(..., AT_CPU, 0)`, la misma aritmética `fb_phys - bar1_phys` que ya usan `ce_fill_fb`/`ce_blit`), y ese `(phys_addr, size)` se registra en `drivers/src/scheme/gem_mmap.rs` bajo el propio handle nouveau (rango alto, `0x8000_0001+`, para no colisionar con la tabla de handles genérica de `linux-object`). Un `mmap()` del fd de la tarjeta con ese offset ahora mapea la VRAM real — ver "Qué probar primero en hardware real". Si `gem_map_cpu` falla (no debería, dado que `GEM_NEW` ya exige `DOMAIN_VRAM`), `map_handle` queda en 0 — el objeto sigue siendo válido para `VM_BIND`/`EXEC`, solo no mmap-able, igual que nouveau real deja `map_handle` ausente para dominios no mapeables |
| `DRM_IOCTL_NOUVEAU_GEM_PUSHBUF` | ❌ | ruta legacy pre-`VM_BIND`, no aplica al modelo que se está siguiendo aquí |
| `DRM_IOCTL_NOUVEAU_GEM_CPU_PREP` / `CPU_FINI` | 🟡 | Solo valida que el handle existe — no consultan los syncobjs de `EXEC` (que sí existen ahora, ver abajo), así que un `CPU_PREP` justo después de un `EXEC` que toque ese buffer sigue sin ser seguro de confiar. Esto era casi un no-op mientras `mmap()` no era real (nada que leer/escribir desde CPU); ahora que `GEM_NEW` puede dar un `map_handle` real, una carrera CPU-vs-GPU a través de ese mapeo durante un `EXEC` en vuelo es un riesgo concreto, no solo teórico — ver "Huecos conocidos" |
| `DRM_IOCTL_NOUVEAU_GEM_INFO` | 🟡 | `size` siempre real. **`map_handle` real** (igual que `GEM_NEW`, mismo mecanismo). **`offset` real** cuando `VM_BIND` ya mapeó el objeto (cruza `nouveau_vm_mappings` por `gem_handle`); sigue en 0 si aún no hay `VM_BIND` |

## DRM syncobjs (`drm_syncobj`)

Core de DRM, no específico de nouveau — su rango de ioctl (`0xBF`-`0xCF`)
va POR ENCIMA de `DRM_COMMAND_END` (0xA0), a diferencia de los ioctls
privados de driver, así que a diferencia de los números de la uAPI de
nouveau estos nunca llevan el offset `DRM_COMMAND_BASE`. El estado vive en
`drivers::scheme::syncobj` (no en `linux-object`, donde se despachan los
ioctls) precisamente para que el propio camino de envío de un driver — el
`EXEC` de la uAPI de nouveau, aquí — pueda señalar un syncobj directamente
tras una finalización real de hardware, sin tener que llamar hacia arriba
a una capa superior (esta parte del árbol de crates no depende de
`linux-object`, y el orden de capas solo permite llamar hacia abajo).

| ioctl | Estado | Notas |
|---|---|---|
| `DRM_IOCTL_SYNCOBJ_CREATE` / `DESTROY` | ✅ | Contabilidad pura (`Vec` + contador de handles), sin acceso a hardware |
| `DRM_IOCTL_SYNCOBJ_RESET` / `SIGNAL` | ✅ | Señal binaria = punto de timeline 1 |
| `DRM_IOCTL_SYNCOBJ_TIMELINE_SIGNAL` / `QUERY` | ✅ | Monótono: una señal nunca mueve el punto hacia atrás (semántica real de `drm_syncobj`) |
| `DRM_IOCTL_SYNCOBJ_WAIT` / `TIMELINE_WAIT` | 🟡 | Real, pero por **sondeo (spin-poll) acotado**, no una cola de espera real: `io_control` (`linux-object/src/fs/devfs/drm_scheme.rs`) es una función síncrona, no `async`, así que no hay forma más barata de bloquear aquí sin cirugía mayor al scheduler. Una espera larga ocupa el core de CPU que atiende el ioctl durante toda su duración. `timeout_nsec` se trata como deadline **absoluto** de `CLOCK_MONOTONIC` (semántica real de Linux, confirmada contra el propio `now_monotonic()` de este kernel) |
| `DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD` / `FD_TO_HANDLE` | ❌ | exportar/importar entre procesos vía fd — fuera de alcance |
| `DRM_IOCTL_SYNCOBJ_TRANSFER` | ❌ | copiar un punto entre dos timelines — caso raro, no implementado |
| `DRM_IOCTL_SYNCOBJ_EVENTFD` | ❌ | necesita un eventfd/interrupción real |

Igual que el resto de este trabajo, gateado tras `nvidia.nouveau_uapi`
(`DRM_CAP_SYNCOBJ`/`SYNCOBJ_TIMELINE` en `GET_CAP` reportan 0 sin el flag,
1 con él — para que ningún otro driver/cliente que nunca tocó este trabajo
vea un bit de capacidad cambiar por defecto).

**Qué prueba de verdad una señal de `EXEC`**: `eclipse_rm_exec_submit_signaled`
añade una SEGUNDA entrada GP justo después de la del caller — un único
`RELEASE` de semáforo host escrito por el kernel en un offset fijo del
buffer del PROPIO canal (nunca el del caller), con timbre único para
ambas entradas. Como GPFIFO procesa en orden estricto, una señal que
aterriza prueba que PBDMA/HOST ya obtuvo y procesó la entrada del
caller — **no prueba por sí sola que el motor de cómputo/GR terminó de
ejecutarla** (eso necesitaría un semáforo de dominio de motor, como el
segundo de `step18`, coordinado con la clase de motor que el propio
contenido del caller haya enlazado — algo que esta función genérica no
puede asumir con seguridad). Una prueba real de finalización de motor
(el equivalente a un `dma_fence` de verdad) es trabajo de seguimiento.

## Huecos conocidos y qué se necesita para cerrarlos

- **`EXEC` con `wait_count == 1` espera por CPU, no por hardware**: bloquea
  la propia llamada al ioctl (con `crate::scheme::syncobj::wait`, timeout
  fijo de 1 s) hasta que el syncobj de espera señale, y SOLO ENTONCES
  somete el *pushbuffer* del caller. El contrato observable para un
  caller síncrono es el mismo que el real ("este `EXEC` no empieza a
  ejecutar antes de que la fence de espera señale"), pero el mecanismo
  interno es distinto: el nouveau real hace que el propio canal de
  hardware ejecute un método `ACQUIRE` de semáforo antes del contenido
  del caller, de modo que la llamada de envío vuelve de inmediato y
  varios envíos dependientes pueden solaparse en el tiempo. Aquí NO
  pueden solaparse — cada `EXEC` con espera ocupa el hilo que hizo el
  ioctl hasta que su propia espera se resuelve. Un `ACQUIRE` real de
  hardware sería una pieza nueva de RM (un método más en el *pushbuffer*
  del canal, antes del contenido del caller) — no hecha aquí.
- **`EXEC` con `sig_count` > 1 o `wait_count` > 1**: un solo syncobj de
  espera y uno de señal por envío. Más
  de uno es iterar el mismo patrón — riesgo bajo, no hecho todavía.
- **`SYNCOBJ_WAIT`/`TIMELINE_WAIT` por sondeo, no cola de espera real**:
  ver la tabla de arriba — ocupa un core de CPU durante la espera.
- **Sin fd export (`HANDLE_TO_FD`/`FD_TO_HANDLE`)**: un syncobj no puede
  compartirse entre procesos ni con una `sync_file` del kernel.
- **Un solo `op`/`push` por llamada**: `VM_BIND` y `EXEC` reales de
  nouveau aceptan arreglos (`op_count`/`push_count` > 1) para agrupar
  varias operaciones en una sola syscall. Aquí se exige exactamente 1;
  más de uno devuelve `EOPNOTSUPP`. Extenderlo es iterar el arreglo con
  el mismo camino ya construido — riesgo bajo, solo no se hizo todavía.
- **`GEM_CLOSE` no limpia mapeos `VM_BIND` pendientes**: cerrar un handle
  que sigue mapeado (`VM_BIND` `MAP` sin `UNMAP` previo) libera el
  `hMemory` en RM vía `nouveau_gem_close` pero deja la entrada
  correspondiente en `nouveau_vm_mappings` — su reserva de VA (`h_virt`)
  nunca se libera en RM, y una `VM_BIND` `UNMAP` posterior sobre esa
  entrada operaría sobre una VA cuya memoria física de respaldo ya no
  existe. El nouveau real también espera `UNMAP` antes de `CLOSE` por
  contrato de userspace, pero limpia igual del lado del kernel; aquí no
  — ningún otro camino de desmontaje de este driver (p. ej.
  `CHANNEL_FREE`) limpia `VM_BIND` tampoco, así que esto es consistente
  con el resto, no una regresión nueva, pero sigue siendo un hueco real.
- **`CPU_PREP`/`CPU_FINI` no esperan de verdad**: ahora que `map_handle`
  puede ser real (ver arriba), un `CPU_PREP` no bloquea hasta que el
  último `EXEC` sobre ese buffer termine — solo valida que el handle
  existe. Cerrar esto necesita fencing implícito por-buffer (qué
  syncobj/fence fue el último en tocar cada `hMemory`), que ni `EXEC` ni
  `GEM_NEW` llevan hoy — sería una pieza nueva, no una extensión de lo
  que ya existe.
- **`CHIPSET_ID` real**: hoy es el mínimo del rango `PMC_BOOT0` de la
  arquitectura ya identificada por PCI ID, no una lectura en vivo de
  `PMC_BOOT0`. Deliberado: cualquier lectura de registro nueva en el
  camino de un ioctl que Mesa puede llamar en cualquier momento del boot
  necesita la misma cautela que ya se documenta en `DrmScheme::debug_dump`
  ("un BAR0 access temprano puede colgar algunas GPUs").
- **Más de un canal**: `CHANNEL_ALLOC` sigue dependiendo de `step16`/
  `step17`, que construyen siempre la misma escalera fija — no aceptan
  "otro cliente, otro TSG". Sin cambios desde el hito anterior.
- **Anillo GPFIFO compartido con `step18`/`step19`**: `eclipse_rm_exec_submit`
  lee `GPPut`/`GPGet` en vivo del USERD (no un contador propio en Rust),
  así que no debería pisar entradas que `step18`/`step19` ya escribieron
  en el mismo arranque — pero esa interacción nunca se ha probado en
  hardware real. Si vas a experimentar con `EXEC`, ten en cuenta que
  también corrieron `/proc/gpustep18`/`19` en el mismo boot.

## Qué probar primero en hardware real

Con `nvidia.nouveau_uapi` activo y la GPU ya atacada al RM (`/proc/gpustep5`
`;6;8;9`, o `gpustep14` en la GPU de consola):

1. `GETPARAM` con `PCI_VENDOR`/`PCI_DEVICE`/`FB_SIZE` — sin acceso a
   hardware, debería funcionar siempre; si falla, el bug está en el
   despacho de ioctls, no en RM.
2. `CHANNEL_ALLOC` — reusa `step16`+`step17`; si esos dos ya funcionan por
   `/proc/gpustepN`, esto solo debería envolver el mismo resultado en la
   forma de nouveau. Confirmar que `hVas`/`hNotifier` en el log coinciden
   con lo que ya reportan esos endpoints.
3. `GEM_NEW` con `DOMAIN_VRAM` y un tamaño pequeño (p. ej. 4096) — ahora
   una asignación real del heap de RM; confirmar en el log el `hMemory`
   devuelto y que la asignación no interfiere con nada más que ya use RM.
4. `VM_INIT` después de `CHANNEL_ALLOC` — debería aceptar y devolver
   0/0; antes de `CHANNEL_ALLOC` debería fallar con `EINVAL`.
5. `VM_BIND` (`MAP`) del handle de (3) a una VA elegida (p. ej.
   `0x7000_0000_0000`, alineada a página) — confirmar en el log
   `virtStatus`/`mapStatus` y que `actualVA` coincide con lo pedido.
6. `VM_BIND` (`UNMAP`) de la misma región — confirmar que no deja el
   VAS en un estado raro para un `MAP` posterior.
7. **El experimento de mayor riesgo**: escribir a mano (desde otro
   programa, vía `mmap` de un `CREATE_DUMB` genérico + `PRIME` hacia el
   GEM real, o algún otro camino CPU-visible) un método simple (p. ej.
   una única `RELEASE` de semáforo host, igual que hace `step18`) en el
   buffer mapeado por (5), y luego `EXEC` con `sig_count=0` apuntando a
   esa VA — confirmar que el semáforo aparece. Esto es lo que de verdad
   prueba que el camino genérico (no el hardcodeado de `step18`) funciona.
8. `SYNCOBJ_CREATE` (sin `DRM_SYNCOBJ_CREATE_SIGNALED`) — confirmar que
   `SYNCOBJ_QUERY` devuelve punto 0.
9. `EXEC` del mismo pushbuffer que (7), esta vez con `sig_count=1`
   apuntando al syncobj de (8) — confirmar en el log que
   `fenceSubmitStatus`/`fenceWaitStatus` son `NV_OK` y que
   `SYNCOBJ_QUERY` ahora devuelve punto 1 (o el `timeline_value` pedido,
   si es un syncobj de timeline).
10. `SYNCOBJ_WAIT` sobre un syncobj YA señalado — debe volver
    inmediatamente. Sobre uno sin señalar con un `timeout_nsec` corto
    (deadline absoluto de `CLOCK_MONOTONIC`, no relativo) — debe volver
    con error tras el timeout, no colgarse.
11. `EXEC` con `wait_count=1` apuntando a un syncobj SIN señalar —
    confirmar que el ioctl se queda bloqueado (no que falla de
    inmediato) y que si otro hilo/proceso lo señala (`SYNCOBJ_SIGNAL`)
    antes de 1 s, el `EXEC` recién entonces somete el pushbuffer y
    vuelve `NV_OK`. Repetir sin señalarlo nunca — debe volver `EIO`
    tras ~1 s, y el pushbuffer NO debe haberse sometido (verificar que
    `GPPut` de la USERD no avanzó).
12. `GEM_NEW` con `DOMAIN_VRAM` — confirmar en el log que
    `gem_map_cpu` devuelve `lookupStatus=0 addressSpace=0` y un
    `map_handle` distinto de 0. Luego `mmap(fd_tarjeta, size,
    PROT_READ|PROT_WRITE, MAP_SHARED, fd, map_handle)` desde el mismo
    proceso: debe devolver un puntero válido, no fallar con `EINVAL`.
13. Escribir un patrón conocido a través del puntero de (12), y
    releer — confirma que el mapeo apunta a memoria real y estable
    (no una página cualquiera reutilizada). Si hay forma de leer VRAM
    por otro camino (p. ej. `VM_BIND` + `EXEC` con un kernel que
    copie a un buffer de verificación), comparar contra eso para
    confirmar que es la MISMA VRAM que ve la GPU, no una copia.
14. `GEM_CLOSE` (ioctl genérico, no `DRM_IOCTL_NOUVEAU_*`) sobre el
    handle de (12) — confirmar en el log `nouveau_gem_close` con
    `gem_free status=Some(0x0)`, y que un `mmap()` posterior con el
    MISMO `map_handle` ya falla (la entrada en `gem_mmap` se quitó).
    Repetir `GEM_CLOSE` sobre el mismo handle una segunda vez — debe
    fallar (`EINVAL`), no repetir el `gem_free`.

## Mapa de archivos

| Archivo | Rol |
|---|---|
| `drivers/src/display/nouveau_uapi.rs` | números de ioctl, structs (layout C exacto de `nouveau_drm.h`), flag opt-in |
| `drivers/src/display/nvidia.rs` (`NvidiaGpu::ioctl` → `nouveau_ioctl`) | despacho real |
| `drivers/src/scheme/syncobj.rs` | estado y sondeo de los DRM syncobjs — genérico, sin acceso a hardware |
| `drivers/src/scheme/gem_mmap.rs` | registro `handle -> (phys_addr, size)` para objetos GEM privados de un driver (hoy: nouveau `GEM_NEW`) que necesitan ser mmap-ables por el mismo mecanismo de offset falso que ya usa `CREATE_DUMB` |
| `nvidia-rm-sys/vendor/eclipse_rm_init.c` (`eclipse_rm_gem_alloc_vram`/`gem_free`/`vm_bind_map`/`vm_bind_unmap`/`exec_submit`/`exec_submit_signaled`) | las primitivas RM genéricas, modeladas línea a línea sobre `step16`-`step19` (que sí corrieron en hardware real) |
| `nvidia-rm-sys/src/rm_init.rs` (`gem_alloc_vram`/`gem_free`/`vm_bind_map`/`vm_bind_unmap`/`exec_submit`/`exec_submit_signaled`) | wrappers Rust seguros sobre lo anterior |
| `linux-object/src/fs/devfs/drm_scheme.rs` | despacho de los ioctls `SYNCOBJ_*` (core DRM, no nouveau-específico) y de `GET_CAP` para `DRM_CAP_SYNCOBJ*` |
| `kernel-hal/src/drivers.rs` (`set_nouveau_uapi_enabled`) | puente para que `zCore` active el flag sin depender directamente de `zcore-drivers` |
| `zCore/src/main.rs` | lee `nvidia.nouveau_uapi` de la cmdline |
