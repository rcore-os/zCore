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
| `DRM_IOCTL_NOUVEAU_GETPARAM` | ✅ | `PCI_VENDOR`/`PCI_DEVICE`/`FB_SIZE`/`VRAM_BAR_SIZE` reales; `CHIPSET_ID` es el mínimo del rango `PMC_BOOT0` de la arquitectura ya identificada por PCI ID — suficiente para que Mesa elija la familia (Turing), que la selecciona por `chipset & ~0xf` (0x160 == 0x166 en ese cálculo); `PTIMER_TIME` devuelve un contador monótono de ns derivado de CPU (stand-in honesto para `GL_TIMESTAMP`, sin leer BAR0); `EXEC_PUSH_MAX` = 64 (el tope real del `EXEC` de este driver); `GRAPH_UNITS` devuelve **EINVAL** a propósito (su valor real es la topología GPC/TPC/MP floorswept — falsearlo mal infra-dimensiona el TLS de shaders y cuelga la GPU; necesita una query de topología al RM, pendiente); `VRAM_USED` siempre 0 |
| **DRM `VERSION` name = `"nouveau"` (v1.4.0)** | ✅ | Con `nvidia.nouveau_uapi` activo, `DRM_IOCTL_VERSION` reporta el nombre de driver **`nouveau`** y versión **1.4.0** (`linux-object/src/fs/devfs/drm_scheme.rs`), en vez de `"zcore"`. **Crítico**: Mesa elige el DRI driver (`nouveau_dri.so`) por ese nombre — con `"zcore"` no cargaba nada. La versión 1.4.0 es la que en nouveau real habilita la uAPI de envío NUEVA (`VM_INIT`/`VM_BIND`/`EXEC`), que es justo la que este driver implementa (no la legacy `GEM_PUSHBUF`). Sin el flag sigue siendo `"zcore"` (el path virtio-gpu de QEMU no se toca) |
| `DRM_IOCTL_NOUVEAU_SETPARAM` | ❌ | deprecated incluso en Linux; no implementado |
| `DRM_IOCTL_NOUVEAU_CHANNEL_ALLOC` | 🟡 | Reusa la escalera `step16`+`step17` ya existente. **Un solo canal en todo el sistema** — un segundo `CHANNEL_ALLOC` sin `CHANNEL_FREE` antes devuelve `EBUSY`. Los campos legacy (`fb_ctxdma_handle`, `subchan[]`) se ignoran: no aplican a Turing+ |
| `DRM_IOCTL_NOUVEAU_CHANNEL_FREE` | 🟡 | La escalera `step16`/`step17` en sí no se desmonta — `nvidia-rm-sys` no tiene un punto de desmontaje real para ella (su propio doc la llama "idempotente", pensada para construirse una vez por arranque); un `CHANNEL_ALLOC` posterior reutiliza la misma asignación cacheada, no crea una nueva. **Sí drena de verdad** `nouveau_vm_mappings` (`drain_vm_mappings`, mismo camino que `GEM_CLOSE`) — como este driver modela un solo VAS global, liberar el canal desmapea (`vm_bind_unmap` real) TODO lo que seguía mapeado, para que el próximo `CHANNEL_ALLOC` empiece de una VM vacía en vez de heredar mapeos de la sesión anterior |
| `DRM_IOCTL_NOUVEAU_NVIF` | ❌ | no implementado |
| `DRM_IOCTL_NOUVEAU_SVM_INIT` / `SVM_BIND` | ❌ | memoria unificada CPU/GPU — fuera de alcance de este hito |
| `DRM_IOCTL_NOUVEAU_VM_INIT` | 🟡 | Exige un canal ya asignado (`CHANNEL_ALLOC` primero, igual que en Linux real). Devuelve un rango `kernel_managed` vacío (0/0) — placeholder honesto: nada reserva ese rango todavía |
| `DRM_IOCTL_NOUVEAU_VM_BIND` | 🟡 | **Real**: `eclipse_rm_vm_bind_map`/`unmap` (nuevo en `eclipse_rm_init.c`) generalizan el patrón de `step17` (reservar VA en `hVas` + `Map`) para un handle GEM y dirección elegidos por el caller. **`op_count` real, de 1 a 64** por llamada (`op_ptr` como arreglo de `DrmNouveauVmBindOp`; ver "Huecos conocidos" sobre no-atomicidad entre ops) — 0 o más de 64: `EINVAL`/`EOPNOTSUPP`. **`wait_count == sig_count == 0`** siempre exigido (esperar/señalar aquí no tendría sentido — no hay trabajo de GPU que sincronizar, solo (des)mapeo de VA) |
| `DRM_IOCTL_NOUVEAU_EXEC` | 🟡 | **Real**: `eclipse_rm_exec_submit` generaliza la mecánica de `step18` (GP entry + `GPPut` + timbre) para un *pushbuffer* `(va, len)` que el caller ya escribió. **`push_count` real, de 1 a 64** — cada *pushbuffer* se somete en orden; si `sig_count > 0` el fence del kernel se ata solo al último (GPFIFO es estrictamente ordenado, así que una señal ahí prueba que TODOS los anteriores también se obtuvieron). `sig_count == 0` (fire-and-forget) o **`sig_count` real, de 0 a 64** DRM syncobjs (ver sección de syncobjs) — `eclipse_rm_exec_submit_signaled` añade una segunda entrada GP con un semáforo propio del kernel tras el último *push* y solo marca los syncobjs (todos, no atómico — ver "Huecos conocidos") tras confirmar que aterrizó. **`wait_count` real, de 0 a 64**, pero por **espera de CPU antes de someter** (`crate::scheme::syncobj::wait` con `wait_all=true`, timeout fijo de 1 s para el arreglo completo), NO por un `ACQUIRE` de semáforo ejecutado por el propio canal de hardware — ver la nota en "Huecos conocidos" |
| `DRM_IOCTL_NOUVEAU_GET_ZCULL_INFO` | ❌ | no implementado |
| `DRM_IOCTL_NOUVEAU_GEM_NEW` | 🟡 | Solo `NOUVEAU_GEM_DOMAIN_VRAM` (memoria de sistema/GART: `EOPNOTSUPP`). **Reserva real vía el heap del RM** (`eclipse_rm_gem_alloc_vram`, clase `NV01_MEMORY_LOCAL_USER` — la misma que usa `step17` para USERD), no un allocador Rust paralelo que podría chocar con la contabilidad propia de RM sobre la misma VRAM. `offset` (VA de GPU) es 0 hasta que `VM_BIND` lo mapea. **`map_handle` real**: `eclipse_rm_gem_map_cpu` (nuevo en `eclipse_rm_init.c`) resuelve el `hMemory` recién asignado a su offset BAR1-relativo real (`memGetByHandle` + `memdescGetPhysAddr(..., AT_CPU, 0)`, la misma aritmética `fb_phys - bar1_phys` que ya usan `ce_fill_fb`/`ce_blit`), y ese `(phys_addr, size)` se registra en `drivers/src/scheme/gem_mmap.rs` bajo el propio handle nouveau (rango alto, `0x8000_0001+`, para no colisionar con la tabla de handles genérica de `linux-object`). Un `mmap()` del fd de la tarjeta con ese offset ahora mapea la VRAM real — ver "Qué probar primero en hardware real". Si `gem_map_cpu` falla (no debería, dado que `GEM_NEW` ya exige `DOMAIN_VRAM`), `map_handle` queda en 0 — el objeto sigue siendo válido para `VM_BIND`/`EXEC`, solo no mmap-able, igual que nouveau real deja `map_handle` ausente para dominios no mapeables |
| `DRM_IOCTL_NOUVEAU_GEM_PUSHBUF` | 🟡 | **Corrección de rumbo (importante)**: una investigación de la imagen mostró que el Mesa que se instala es **solo el stack Gallium clásico** (`mesa-dri-gallium` + `mesa-gl`); para Turing eso es el driver **nvc0**, que somete OpenGL por **esta** ioctl (`GEM_PUSHBUF`), NO por la uAPI nueva `VM_BIND`/`EXEC` (esa la usa NVK/Vulkan, que la imagen no trae). Es decir, esta era la ruta a implementar desde el principio para el OpenGL real de la imagen. Hoy se **parsea y vuelca** (canal, `nr_buffers`/`nr_relocs`/`nr_push`, dominios de cada BO, `(bo_index, offset, length)` de cada push — prefijo acotado a 8 por arreglo), y luego devuelve **`EOPNOTSUPP`** honesto. El volcado es la anatomía necesaria para el envío real, que necesita: (a) GEM de dominio **GART** (memoria de sistema mapeada al VAS de GPU; hoy `GEM_NEW` es solo VRAM), (b) la **clase 3D** (`TURING_A` 0xc597) atada al canal (hoy `CHANNEL_ALLOC` solo monta la de cómputo 0xc5c0 vía step16/17), y (c) resolución de **relocs** — todo trabajo de seguimiento validado en hardware, nunca simulado |
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
| `DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD` / `FD_TO_HANDLE` | 🟡 | **Real**, pero despachado en `linux-syscall` (`sys_drm_syncobj_fd`), NO en `io_control` — son las únicas syncobj ioctls que necesitan la tabla de fd del proceso, igual que `PRIME_HANDLE_TO_FD`/`FD_TO_HANDLE`. Como la tabla de syncobjs es un espacio de handles GLOBAL (no por proceso), "exportar" no mueve ni copia nada — el número de handle ya es válido globalmente, el fd solo lo transporta. `SYNCOBJ_*_FLAGS_IMPORT_SYNC_FILE` (interoperar con un `sync_file` POSIX real) devuelve `EOPNOTSUPP` — Eclipse no tiene esa abstracción |
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

## Reclamo al salir el proceso

`linux-object` ya tenía un hook de salida de proceso
(`zircon_object::task::set_process_exit_hook`, en
`linux-object/src/fs/mod.rs`) que libera los `CREATE_DUMB`/PRIME de un
proceso que muere sin `DESTROY_DUMB`/`GEM_CLOSE` (`drm::release_process`).
Nada equivalente existía para el estado privado de nouveau — un cliente
que se cae (o lo mata el compositor) sin `CHANNEL_FREE`/`GEM_CLOSE`
fugaba el canal y toda su VRAM real hasta el próximo reinicio.

**El problema de fondo**: `drivers` (donde vive `NvidiaGpu`) no puede
saber qué proceso está haciendo una llamada — no depende de
`kernel-hal`/`zircon-object` (`get_current_thread()` de `kernel-hal`
devuelve un `Arc<dyn Any>` opaco a propósito; solo capas por encima,
que sí conocen el tipo concreto `zircon_object::task::Thread`, pueden
convertirlo en un pid). `linux-object` sí lo sabe (ya lo usa para
`release_process`), así que el pid se empuja hacia abajo en vez de
intentar que `drivers` lo averigüe:

- `DrmScheme` gana `ioctl_owned(request, arg, owner_pid)` (default:
  ignora `owner_pid` y llama a `ioctl` — CERO impacto en cualquier otro
  driver, p. ej. `virtio-gpu`). `drm_scheme.rs`'s despacho de ioctls
  desconocidos ahora llama `ioctl_owned(cmd, data, drm::current_pid())`
  en vez de `ioctl(cmd, data)` directo.
- `NvidiaGpu::ioctl` (el método del trait, todavía necesario porque
  algunas rutas lo llaman sin conocer un pid) pasa a ser un envoltorio
  fino sobre `ioctl_owned(request, arg, 0)`; toda la lógica real vive
  ahora en `ioctl_owned`, que le pasa `owner_pid` a `nouveau_ioctl`.
- `CHANNEL_ALLOC` guarda ese `owner_pid` en `NouveauChannelState`.
- `DrmScheme` gana `nouveau_release_process(pid)` (default: no-op).
  `NvidiaGpu` la implementa: si el pid que sale coincide con el dueño
  del canal, drena TODOS los `VM_BIND` (`drain_vm_mappings`, igual que
  `CHANNEL_FREE`), libera TODOS los objetos `nouveau_gem` (`gem_free`
  real + `gem_mmap::unregister` de cada uno) y limpia el canal — el
  mismo efecto que un `CHANNEL_FREE` completo, disparado por la salida
  del proceso en vez de por un ioctl explícito.
- El hook de salida (`drm_release_on_exit`, `linux-object/src/fs/mod.rs`)
  ahora llama también a `driver.nouveau_release_process(pid)` junto al
  `release_process(pid)` genérico que ya tenía.

**Qué NO cubre**: si el pid que sale nunca hizo `CHANNEL_ALLOC` con esta
uAPI (`owner_pid` en el canal no coincide, o no hay canal), no pasa
nada — correcto, ese proceso no tenía nada que reclamar aquí. Un
`GEM_NEW` hecho por un pid DISTINTO al dueño del canal (posible hoy,
ya que `GEM_NEW` no exige que el llamador sea el mismo que hizo
`CHANNEL_ALLOC`) tampoco se libera si SU proceso muere — solo se libera
si muere el dueño del canal. Dado que este driver modela un solo canal
global, en la práctica hay un único cliente real a la vez, así que este
caso límite es principalmente teórico.

## Huecos conocidos y qué se necesita para cerrarlos

- **`EXEC` con `wait_count > 0` espera por CPU, no por hardware**: bloquea
  la propia llamada al ioctl (con `crate::scheme::syncobj::wait`,
  `wait_all=true`, timeout fijo de 1 s para el arreglo completo) hasta
  que TODOS los syncobjs de espera señalen, y SOLO ENTONCES somete el
  *pushbuffer* del caller. El contrato observable para un
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
- **`SYNCOBJ_WAIT`/`TIMELINE_WAIT` por sondeo, no cola de espera real**:
  ver la tabla de arriba — ocupa un core de CPU durante la espera.
- **`HANDLE_TO_FD`/`FD_TO_HANDLE` sin refcounting real**: la tabla de
  syncobjs no lleva conteo de referencias — `SYNCOBJ_DESTROY` borra la
  entrada sin importar cuántos fds exportados sigan vivos. En DRM real,
  un fd exportado mantiene vivo el objeto del kernel más allá de un
  `SYNCOBJ_DESTROY` sobre el handle que lo exportó; aquí, tras destruir
  el handle, un `SYNCOBJ_FD_TO_HANDLE` posterior sobre un fd ya
  exportado devuelve un número de handle que simplemente ya no está en
  la tabla — el mismo error "handle desconocido" que ya da `WAIT`/
  `SIGNAL`/`QUERY` para cualquier otro handle inválido. No es un
  cuelgue ni un crash, pero es una vida útil más corta que la real.
  Sin interoperabilidad con `sync_file` POSIX (`IMPORT_SYNC_FILE`
  devuelve `EOPNOTSUPP` — Eclipse no tiene esa abstracción).
- **`VM_BIND` con `op_count` > 1 no es atómico**: cada op se aplica en
  orden con su propia llamada real a RM; si `op[i]` falla, `op[0..i]`
  ya se aplicaron y quedan así, y `op[i+1..]` nunca corren. Coincide
  con cómo se comporta el `VM_BIND` real de nouveau (cada op se valida
  y aplica según se procesa, no como una transacción todo-o-nada), pero
  vale la pena tenerlo presente al depurar un fallo a mitad de arreglo.
- **`EXEC` con `sig_count` > 1 tampoco es atómico al señalar**: si el
  syncobj `i` tiene un handle inválido, los syncobjs antes de `i` ya
  quedaron señalados y los de después de `i` nunca se intentan — mismo
  comportamiento que un solo handle malo ya tenía antes de este hito,
  solo que ahora hay más de uno que puede fallar.
- **`CHANNEL_FREE` explícito no libera `hMemory`**: `CHANNEL_FREE` y
  `GEM_CLOSE` comparten `drain_vm_mappings` (`nvidia.rs`) para soltar
  las reservas de VA (`h_virt`) de cualquier `VM_BIND` que quedara vivo,
  pero `CHANNEL_FREE` en sí NO toca `nouveau_gem` — los objetos GEM (y
  su `hMemory` en el heap del RM) siguen asignados aunque ya no estén
  mapeados en ningún VAS. Correcto en el sentido de que `CHANNEL_FREE`
  real de nouveau tampoco libera objetos GEM del cliente (son recursos
  independientes), así que esto no se "arregla" — pero significa que un
  cliente que llama `CHANNEL_FREE` y sigue vivo sin nunca llamar
  `GEM_CLOSE` mantiene esa VRAM asignada hasta que el proceso termine
  (ver "Reclamo al salir el proceso" abajo, que sí cubre ese caso final).
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

### Paso 0 — descubrimiento/identidad (ANTES de cualquier ioctl)

NVK (el Vulkan de Mesa por el que pasa Zink→GL) **no toca ni un ioctl
nouveau** hasta haber *descubierto* el nodo DRM vía libdrm y haberlo
aceptado. Su filtro de candidatos es, en orden: nodo `renderD*` presente,
`bustype == DRM_BUS_PCI`, y **`vendor_id == 0x10de`** — todo leído de sysfs
(`/sys/dev/char/226:128/device/{vendor,subsystem,…}`) *antes* de abrir el
dispositivo. Si cualquiera falla, NVK salta el nodo y
`vkEnumeratePhysicalDevices` devuelve 0 GPUs **sin una sola traza nouveau**
(porque `nouveau_ws_device_new`, que emite `VERSION`/`GETPARAM`, nunca se
llama). Un `vulkaninfo` con "Failed to detect any valid GPUs" + `dmesg |
grep nouveau` vacío = fallo aquí, no en la uAPI.

Trampa concreta ya vista: el respaldo PCI del nodo DRM elegía "el primer
dispositivo de clase 0x03". En cualquier placa con **otro dispositivo de
vídeo que enumere antes que la RTX** — una iGPU integrada, o (típico en
placas workstation/servidor con 2 GPUs NVIDIA) una **VGA de gestión onboard
del BMC** (ASPEED `0x1a03`, Matrox `0x102b`) — ese "primer 0x03" NO es la
NVIDIA → sysfs anunciaba el vendor equivocado → NVK descartaba el nodo.
Nota: el escaneo PCI de sysfs (`scan_pci_devices`) usa el MISMO
`scan_bus(PortOpsImpl, PCI_ACCESS)` que el probe que ya ató el driver, así
que las GPUs SÍ están en la lista — el fallo era la *elección*, no la
ausencia. Corregido: con la uAPI activa se prefiere el dispositivo de clase
0x03 **con vendor `0x10de`** (`display_pci_index` en
`linux-object/src/fs/sysfs.rs`).

Pendiente/afinado posible: con 2 GPUs NVIDIA el nodo respalda la *primera*
NVIDIA por bus, mientras que `get_primary_driver()` (a quien van los
ioctls) es la *última* sondeada; ambas son NVIDIA con `nouveau_ioctl`
real, así que la enumeración de NVK funciona igual, pero lo correcto sería
respaldar el nodo en la BDF exacta del driver primario (requiere reconciliar
la BDF decimal del nombre del driver con la hex de sysfs).

Diagnóstico en el arranque (nivel `warn`, solo con la uAPI activa):
- `[drm-probe] PCI[i] BDF vendor=… class=…` — inventario PCI completo, y
  `[drm-probe] render node backed by PCI[i] … vendor=…` — a qué GPU apunta
  el nodo (debe ser `0x10de`).
- `[drm] VERSION on /dev/dri/renderD128 … primary_driver="Nvidia GPU"` —
  si aparece, NVK **sí** superó el descubrimiento y llegó a `VERSION`; si
  además `primary_driver` no es `"Nvidia GPU"`, los ioctls driver-private
  se están enrutando al driver equivocado.

Con `nvidia.nouveau_uapi` activo, la GPU ya atacada al RM (`/proc/gpustep5`
`;6;8;9`, o `gpustep14` en la GPU de consola), **y el Paso 0 superado**:

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
15. `GEM_NEW` + `VM_BIND` `MAP` (sin `UNMAP`) + `GEM_CLOSE` directo
    sobre ese handle — confirmar en el log una línea "GEM_CLOSE
    handle=...: dropped stale VM_BIND VA=... -> vm_bind_unmap
    status=0x0" antes del `gem_free`, y que una `VM_BIND` `UNMAP`
    posterior sobre esa misma VA ya falla con `ENOENT` (la entrada se
    drenó, no quedó huérfana).
16. `GEM_NEW` + `VM_BIND` `MAP` + `CHANNEL_FREE` (sin `GEM_CLOSE` ni
    `VM_BIND` `UNMAP` antes) — confirmar en el log "CHANNEL_FREE:
    dropped stale VM_BIND VA=..." para esa VA, y que un `CHANNEL_ALLOC`
    + `VM_INIT` posterior seguido de `GEM_INFO` sobre el mismo handle
    GEM reporta `offset=0` de nuevo (el mapeo realmente se soltó, no
    solo se ocultó) mientras que `map_handle` sigue siendo el mismo
    (el registro de CPU-mmap en `gem_mmap` es independiente de
    `VM_BIND` y `CHANNEL_FREE` no lo toca). El handle GEM en sí debe
    seguir vivo — `GEM_INFO` no debe devolver `ENOENT`.
17. `CHANNEL_ALLOC` + `GEM_NEW` + `VM_BIND` `MAP` desde un proceso, y
    matarlo (`kill -9` o que se caiga solo) SIN llamar `CHANNEL_FREE`
    ni `GEM_CLOSE` — confirmar en el log una línea "process exit
    pid=...: released nouveau channel + N GEM object(s), K KiB", y que
    un `CHANNEL_ALLOC` posterior desde OTRO proceso funciona de
    inmediato (no `EBUSY`, que es lo que devolvería si el canal
    hubiera quedado ocupado). Repetir matando un proceso CUALQUIERA
    que nunca llamó `CHANNEL_ALLOC` — no debe pasar nada (ni logs de
    "released nouveau channel", ni tocar el canal de otro cliente).
18. `VM_BIND` con `op_count=3` (dos `MAP` de handles distintos + un
    `UNMAP` de una VA que NO existe) — confirmar que los dos primeros
    `MAP` de verdad se aplicaron (`GEM_INFO` sobre ambos handles debe
    reportar su `offset`) aunque el tercero devuelva `ENOENT` y el
    ioctl entero falle; el log debe mostrar "op[2] of 3 failed,
    stopping (2 earlier op(s) already applied)".
19. `EXEC` con `push_count=3` y `sig_count=1` — cada *push* debe verse
    en el log ("EXEC pushVA=... -> submitted") en orden, y solo el fence
    del kernel corre tras el ÚLTIMO; confirmar que el syncobj se
    señala solo después de las tres líneas de log, nunca antes.
20. `EXEC` con `wait_count=3` apuntando a tres syncobjs — confirmar que
    el ioctl se queda bloqueado hasta que los TRES estén señalados
    (`wait_all=true`), no solo el primero; señalar dos y dejar uno sin
    señalar debe seguir bloqueando hasta el timeout de 1 s.
21. `VM_BIND` con `op_count=65` o `EXEC` con `push_count=65` — deben
    devolver `EOPNOTSUPP` de inmediato (por encima del límite de 64 de
    este hito), no intentar leer 65 elementos.
22. `SYNCOBJ_CREATE` + `SYNCOBJ_HANDLE_TO_FD` sobre ese handle — el `fd`
    devuelto debe ser un descriptor válido (`fstat` no debe fallar).
    Pasarlo a OTRO proceso (`fork` + heredar, o `SCM_RIGHTS` sobre un
    socket Unix) y desde ahí llamar `SYNCOBJ_FD_TO_HANDLE` — el handle
    resultante debe comportarse como el original: `SYNCOBJ_QUERY`
    reporta el mismo punto, y `SIGNAL`/`WAIT` sobre cualquiera de los
    dos handles (el exportador o el importador) afecta al mismo
    syncobj subyacente. Repetir con `flags=DRM_SYNCOBJ_FD_TO_HANDLE_FLAGS_IMPORT_SYNC_FILE`
    puesto — debe devolver `EOPNOTSUPP`, no interpretar el fd como si
    fuera de los nuestros.

## Mapa de archivos

| Archivo | Rol |
|---|---|
| `drivers/src/display/nouveau_uapi.rs` | números de ioctl, structs (layout C exacto de `nouveau_drm.h`), flag opt-in |
| `drivers/src/display/nvidia.rs` (`NvidiaGpu::ioctl_owned` → `nouveau_ioctl`) | despacho real; también `drain_vm_mappings` y `nouveau_release_process` |
| `drivers/src/scheme/syncobj.rs` | estado y sondeo de los DRM syncobjs — genérico, sin acceso a hardware |
| `drivers/src/scheme/gem_mmap.rs` | registro `handle -> (phys_addr, size)` para objetos GEM privados de un driver (hoy: nouveau `GEM_NEW`) que necesitan ser mmap-ables por el mismo mecanismo de offset falso que ya usa `CREATE_DUMB` |
| `drivers/src/scheme/drm.rs` (`DrmScheme::ioctl_owned`/`nouveau_gem_close`/`nouveau_release_process`) | puntos de extensión del trait para pid del llamador y limpieza de recursos privados de un driver -- default no-op para cualquier driver que no los necesite |
| `nvidia-rm-sys/vendor/eclipse_rm_init.c` (`eclipse_rm_gem_alloc_vram`/`gem_free`/`vm_bind_map`/`vm_bind_unmap`/`exec_submit`/`exec_submit_signaled`) | las primitivas RM genéricas, modeladas línea a línea sobre `step16`-`step19` (que sí corrieron en hardware real) |
| `nvidia-rm-sys/src/rm_init.rs` (`gem_alloc_vram`/`gem_free`/`vm_bind_map`/`vm_bind_unmap`/`exec_submit`/`exec_submit_signaled`) | wrappers Rust seguros sobre lo anterior |
| `linux-object/src/fs/devfs/drm_scheme.rs` | despacho de los ioctls `SYNCOBJ_*` (core DRM, no nouveau-específico), de `GET_CAP` para `DRM_CAP_SYNCOBJ*`, y el fallback a `ioctl_owned` para ioctls no reconocidos |
| `linux-object/src/fs/devfs/drm.rs` (`current_pid`) | resuelve el pid del proceso actual — ya existía para `release_process`, ahora también se usa para `ioctl_owned` |
| `linux-object/src/fs/mod.rs` (`drm_release_on_exit`) | hook de salida de proceso — reclama `CREATE_DUMB`/PRIME (ya existía) y ahora también el estado privado de nouveau (`nouveau_release_process`) |
| `linux-object/src/fs/syncobj_file.rs` (`SyncobjHandle`) | objeto `FileLike` que envuelve un handle de syncobj para `HANDLE_TO_FD`/`FD_TO_HANDLE` — mismo patrón que `dmabuf.rs` para PRIME |
| `linux-syscall/src/file/file.rs` (`sys_drm_syncobj_fd`) | despacha `SYNCOBJ_HANDLE_TO_FD`/`FD_TO_HANDLE` con acceso a la tabla de fd del proceso — igual que `sys_drm_prime` para PRIME, y por la misma razón (`io_control`, a nivel de inodo, no tiene esa tabla) |
| `kernel-hal/src/drivers.rs` (`set_nouveau_uapi_enabled`) | puente para que `zCore` active el flag sin depender directamente de `zcore-drivers` |
| `zCore/src/main.rs` | lee `nvidia.nouveau_uapi` de la cmdline |
