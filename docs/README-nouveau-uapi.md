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
| `DRM_IOCTL_NOUVEAU_VM_BIND` | 🟡 | **Real**: `eclipse_rm_vm_bind_map`/`unmap` (nuevo en `eclipse_rm_init.c`) generalizan el patrón de `step17` (reservar VA en `hVas` + `Map`) para un handle GEM y dirección elegidos por el caller. Limitado a **`op_count == 1`** (más de una operación por llamada: `EOPNOTSUPP`) y **`wait_count == sig_count == 0`** (sin sync objects — ver huecos abajo). `MAP` y `UNMAP` implementados |
| `DRM_IOCTL_NOUVEAU_EXEC` | 🟡 | **Real**: `eclipse_rm_exec_submit` (nuevo) generaliza la mecánica de `step18` (GP entry + `GPPut` + timbre) para un *pushbuffer* `(va, len)` que el caller ya escribió — ya NO ejecuta un kernel fijo. El slot del anillo se lee de `GPPut`/`GPGet` reales del USERD (no un contador propio), así que compone bien con `step18`/`step19` si ya corrieron en el mismo arranque. Limitado a **`push_count == 1`** y **`wait_count == sig_count == 0`** — sin sync objects, esta llamada bloquea hasta tocar el timbre pero el caller no tiene forma real de saber cuándo terminó la GPU. **No apto todavía para Mesa/NVK real** (que exige sync objects) |
| `DRM_IOCTL_NOUVEAU_GET_ZCULL_INFO` | ❌ | no implementado |
| `DRM_IOCTL_NOUVEAU_GEM_NEW` | 🟡 | Solo `NOUVEAU_GEM_DOMAIN_VRAM` (memoria de sistema/GART: `EOPNOTSUPP`). **Reserva real vía el heap del RM** (`eclipse_rm_gem_alloc_vram`, clase `NV01_MEMORY_LOCAL_USER` — la misma que usa `step17` para USERD), no un allocador Rust paralelo que podría chocar con la contabilidad propia de RM sobre la misma VRAM. `offset` (VA de GPU) es 0 hasta que `VM_BIND` lo mapea. `map_handle` siempre 0: el `mmap()` de CPU de estos objetos necesitaría un registro cruzado en la tabla de handles de `linux-object` — ver "Huecos conocidos" |
| `DRM_IOCTL_NOUVEAU_GEM_PUSHBUF` | ❌ | ruta legacy pre-`VM_BIND`, no aplica al modelo que se está siguiendo aquí |
| `DRM_IOCTL_NOUVEAU_GEM_CPU_PREP` / `CPU_FINI` | 🟡 | Solo valida que el handle existe — **no hay *fencing* real** (no hay sync objects), así que un `CPU_PREP` justo después de un `EXEC` que toque ese buffer NO es seguro de confiar todavía |
| `DRM_IOCTL_NOUVEAU_GEM_INFO` | 🟡 | Mismas limitaciones que `GEM_NEW` (`offset`/`map_handle` en 0 hasta que haya `VM_BIND`/registro cruzado) |

## Huecos conocidos y qué se necesita para cerrarlos

- **Sin DRM syncobjs**: nada aquí implementa `DRM_IOCTL_SYNCOBJ_*`
  (creación, timeline, wait/signal) — por eso `VM_BIND`/`EXEC` rechazan
  cualquier `wait_count`/`sig_count` distinto de cero. Sin esto, un
  `EXEC` real es solo "dispara y reza": el caller no tiene manera de
  saber cuándo terminó la GPU. Esto es lo que de verdad falta para que
  Mesa/NVK puedan usar este camino — es una pieza de tamaño comparable a
  todo lo construido en este segundo incremento, no un detalle menor.
- **Un solo `op`/`push` por llamada**: `VM_BIND` y `EXEC` reales de
  nouveau aceptan arreglos (`op_count`/`push_count` > 1) para agrupar
  varias operaciones en una sola syscall. Aquí se exige exactamente 1;
  más de uno devuelve `EOPNOTSUPP`. Extenderlo es iterar el arreglo con
  el mismo camino ya construido — riesgo bajo, solo no se hizo todavía.
- **Sin `GEM_FREE`**: nouveau real libera objetos GEM vía el `GEM_CLOSE`
  genérico de DRM, no un ioctl propio. Ese `GEM_CLOSE` vive en
  `linux-object` y hoy solo conoce la tabla de handles genérica
  (`imported_handles`), no la tabla `nouveau_gem` de este archivo —
  conectar ambas arriesga colisión de namespaces de handle (dos
  contadores independientes) si se hace sin cuidado. Resultado práctico:
  **cada `GEM_NEW` con este flag activo es una fuga de VRAM real hasta
  el próximo reinicio** (antes era solo contabilidad Rust que se
  reseteaba sola; ahora es una asignación real del heap del RM). Aceptable
  para pruebas puntuales, no para uso prolongado.
- **`mmap()` de objetos `GEM_NEW`**: sigue pendiente, sin cambios desde
  antes — necesita un camino para registrar un buffer asignado dentro de
  `drivers` en la tabla de handles que
  `linux-object/src/fs/devfs/drm_scheme.rs::DrmDev::get_vmo` consulta.
  `drivers` es una capa más abajo que `linux-object`, así que esto es un
  cambio de API entre capas, no un one-liner.
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
   buffer mapeado por (5), y luego `EXEC` apuntando a esa VA — confirmar
   que el semáforo aparece. Esto es lo que de verdad prueba que el
   camino genérico (no el hardcodeado de `step18`) funciona.

## Mapa de archivos

| Archivo | Rol |
|---|---|
| `drivers/src/display/nouveau_uapi.rs` | números de ioctl, structs (layout C exacto de `nouveau_drm.h`), flag opt-in |
| `drivers/src/display/nvidia.rs` (`NvidiaGpu::ioctl` → `nouveau_ioctl`) | despacho real |
| `nvidia-rm-sys/vendor/eclipse_rm_init.c` (`eclipse_rm_gem_alloc_vram`/`gem_free`/`vm_bind_map`/`vm_bind_unmap`/`exec_submit`) | las primitivas RM genéricas, modeladas línea a línea sobre `step16`-`step19` (que sí corrieron en hardware real) |
| `nvidia-rm-sys/src/rm_init.rs` (`gem_alloc_vram`/`gem_free`/`vm_bind_map`/`vm_bind_unmap`/`exec_submit`) | wrappers Rust seguros sobre lo anterior |
| `kernel-hal/src/drivers.rs` (`set_nouveau_uapi_enabled`) | puente para que `zCore` active el flag sin depender directamente de `zcore-drivers` |
| `zCore/src/main.rs` | lee `nvidia.nouveau_uapi` de la cmdline |
