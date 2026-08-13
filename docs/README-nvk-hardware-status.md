# NVK (Vulkan) en la RTX real — estado y traspaso

Estado del intento de llevar **Zink→NVK** (OpenGL por Vulkan, driver `nouveau`
de Mesa) a renderizar el escritorio en el hardware real de 2 GPUs NVIDIA.
Escrito como traspaso: qué está resuelto, qué bloquea, y el flujo exacto para
retomarlo cuando se pueda iterar sobre hardware.

## Resumen en una línea

El camino se recorre por capas. Las capas 0–3 están resueltas o con fix; el
bloqueo real que queda es la **capa 4: el RM no se ata automáticamente al
arrancar**, y sin RM atado no hay GEM/EXEC (render). Atarlo automáticamente es
arriesgado a ciegas (el arranque de GSP-RM puede colgar), así que hoy es un
paso **manual** (`cat /proc/gpustep14`).

## Hechos confirmados en hardware

- 2 GPUs NVIDIA, sin iGPU. Registradas como DRM: `nvidia-gpu-23:0.0` (bus
  0x17) y `nvidia-gpu-101:0.0` (bus 0x65). `max_mode=1366x768 3d=true`.
- La cmdline SÍ lleva `renderer=gl:nvidia.nouveau_uapi` (el path nouveau está
  activo) y `LOG=error` (que oculta los logs de nivel `warn`).
- `vulkaninfo` corre (NVK instalado) pero `vkEnumeratePhysicalDevices` → 0
  GPUs.

## Capas

### Capa 0 — empaquetado Vulkan ✅
`vulkan-loader` + `mesa-vulkan-nouveau` (NVK) + `vulkan-tools` en la imagen
(`LIVE_TREES` arreglado para el manifest ICD). `vulkaninfo` ejecuta → NVK carga.

### Capa 1 — cmdline / build ✅ (con footgun arreglado)
`nvidia.nouveau_uapi` (que gatea TODO el path nouveau, el fix del nodo DRM y el
diagnóstico) solo se estampa con **`GL=1`**. El `make iso` raíz no propagaba
`GL` al build del kernel → arreglado: `make iso GL=1` ya deja la cmdline
correcta. **Nota**: `make image`/`img`/`qcow2` NO reconstruyen el kernel; solo
`make iso` lo hace. Y `LOG=error` (default de hardware) oculta `warn`: el
diagnóstico `[drm-probe]` se emite ahora por `klog_info` (visible con
LOG=error), pero las trazas de ioctl nouveau viven en el crate `drivers` (no
pueden usar klog) → para verlas hace falta arrancar con **`LOG=warn`**.

### Capa 2 — descubrimiento DRM (libdrm/NVK) ✅ (con fix)
NVK filtra candidatos por: nodo `renderD*`, `bustype==PCI`, **vendor
`0x10de`** — leído de sysfs, ANTES de abrir el dispositivo. Fix:
`display_pci_index` prefiere el dispositivo clase 0x03 **con vendor 0x10de**, no
"el primer 0x03" (que en placas con VGA de gestión del BMC —ASPEED/Matrox— sería
la equivocada). Diagnóstico en arranque: `dmesg | grep drm-probe`.

### Capa 3 — creación de dispositivo / VM_INIT ✅ (fix clave)
`nouveau_ws_device_new()` de NVK llama **`VM_INIT` lo PRIMERO**, antes de
cualquier `CHANNEL_ALLOC`. Nuestra implementación exigía un canal y devolvía
`EINVAL` → NVK abortaba la creación del dispositivo → 0 GPUs. Corregido:
`VM_INIT` es standalone (acepta sin canal). Esto desbloquea la *enumeración* de
NVK.

### Capa 4 — render (GEM/VM_BIND/CHANNEL/EXEC) ⛔ BLOQUEO ACTUAL
Estas ioctls sí necesitan el RM atado (`rm_device_instance`), que **hoy solo se
pone leyendo `/proc/gpustep14`** (o la escalera 5/6/8/9), NO automáticamente al
arrancar. Sin RM atado, GEM_NEW(VRAM)/VM_BIND/CHANNEL_ALLOC/EXEC fallan
(ENODEV). El compositor arranca antes de que se pueda atar el RM a mano, así que
su init de NVK corre sin RM.

Automatizar el attach al boot es la vía obvia pero **arriesgada a ciegas**: el
arranque de GSP-RM hace bring-up de hardware que puede colgar; por eso la
escalera es manual. No se debe activar sin poder observar el arranque.

### Capa 5 — consistencia con 2 GPUs ⚠️ (pendiente, benigno para enumerar)
El nodo DRM respalda la PRIMERA GPU 0x10de por bus (0x17), pero
`get_primary_driver()` (a quien van los ioctls) es la ÚLTIMA registrada (0x65).
Ambas son NVIDIA con `nouveau_ioctl` real, así que la enumeración funciona
igual; pero lo correcto sería que el nodo respalde exactamente la GPU del driver
primario (y que esa sea la que se ata al RM). Requiere reconciliar la BDF
decimal del nombre del driver con la hex de sysfs.

## Flujo para probar en hardware (cuando se pueda)

```sh
# 1) Build con el flag y trazas visibles
git pull
make iso GL=1 LOG=warn        # kernel+rootfs+ISO; LOG=warn para ver las trazas ioctl
# flashea la ISO, arranca

# 2) Confirmar estado
cat /proc/cmdline                       # debe tener renderer=gl:nvidia.nouveau_uapi
dmesg | grep drm-probe                  # inventario PCI + a qué GPU respalda el nodo
cat /sys/dev/char/226:128/device/vendor # debe ser 0x10de

# 3) Atar el RM (habilita GEM/EXEC) -- HOY es manual
cat /proc/gpustep14 > /r14.txt; sync    # en la GPU de consola
#   revisar /r14.txt: el attach + GSP boot deben completar sin timeout

# 4) Probar NVK ya con RM atado
vulkaninfo 2>&1 | head -20              # ¿enumera la RTX ahora?
dmesg | grep -E "nouveau-uapi|drm\] VERSION"   # hasta dónde llega NVK y qué ioctl falla
```

## Árbol de decisión

- `vulkaninfo` sigue 0 GPUs y `drm-probe` dice vendor≠0x10de → el nodo respalda
  la GPU equivocada; revisar por qué `display_pci_index` no eligió la 0x10de.
- `drm-probe` dice vendor=0x10de y sysfs resuelve, pero 0 GPUs y NINGUNA traza
  `nouveau-uapi first ...` (con LOG=warn) → NVK no abre el nodo: mirar
  `available_nodes`/agrupación de libdrm (card0+renderD128 misma BDF).
- Hay trazas `nouveau-uapi first GETPARAM/VM_INIT ...` pero para en VM_BIND/
  GEM/EXEC → es la capa 4: confirmar que el RM está atado (`gpustep14` OK) y que
  `get_primary_driver()` es la GPU atada.

## Próximos pasos (en orden)

1. **RM auto-attach seguro**: atar el RM de la GPU primaria al arrancar, con
   guardas/timeout para no colgar, y solo tras validar `gpustep14` a mano varias
   veces. Es el desbloqueo real de la capa 4.
2. Consistencia 2-GPU (capa 5): respaldar el nodo en la BDF del driver primario
   y atar el RM de ESA GPU.
3. Repasar qué GETPARAM/param extra pide `nouveau_ws_device_new` de NVK que
   podamos estar devolviendo con EINVAL (arquitectura/chipset correctos).

## Commits de esta línea de trabajo

- Empaquetado Vulkan/NVK + `LIVE_TREES`.
- `display_pci_index` prefiere 0x10de (capa 2).
- `[drm-probe]` diagnóstico + `klog_info` (visible con LOG=error).
- `make iso` propaga `GL`/`DESKTOP` (capa 1).
- `VM_INIT` standalone, sin exigir canal (capa 3, fix clave).
