# NVK (Vulkan) en la RTX real — estado y traspaso

Estado del intento de llevar **Zink→NVK** (OpenGL por Vulkan, driver `nouveau`
de Mesa) a renderizar el escritorio en el hardware real de 2 GPUs NVIDIA.

Todo lo de aquí está **verificado contra el código fuente de la pila que
realmente empaquetamos**, no contra suposiciones: Alpine v3.24 (fijado en
`xtask/src/linux/mod.rs:108`) trae **mesa 26.1.6** y **libdrm 2.4.134**.

## La secuencia real que ejecuta NVK

`vkEnumeratePhysicalDevices` → `drmGetDevices2` → por cada GPU candidata
`nvkmd_nouveau_try_create_pdev` → `nouveau_ws_device_new()`. Los filtros previos
(todos con salto SILENCIOSO a `VK_ERROR_INCOMPATIBLE_DRIVER`) son: nodo
`renderD*` presente, `bustype == PCI`, `vendor == 0x10de`, `open(renderD128,
O_RDWR)`, y `DRM_IOCTL_VERSION` con nombre `"nouveau"` y versión ≥ 1.3.1.

Después, **durante la enumeración** (no al crear el dispositivo lógico):

| # | ioctl | ¿fatal? |
|---|---|---|
| 1 | `VM_INIT` (por `drmCommandWrite`, **`_IOW`**) | sí (marca `has_vm_bind`) |
| 2 | `NVIF NEW` de `NV_DEVICE` (0x0080) | sí |
| 3 | `GETPARAM PCI_DEVICE` (4) | sí |
| 4 | `NVIF MTHD NV_DEVICE_V0_INFO` | sí (única fuente de chipset/VRAM) |
| 5 | `GET_ZCULL_INFO` | no |
| 6 | `GETPARAM EXEC_PUSH_MAX` (17), `VRAM_BAR_SIZE` (18) | no |
| 7 | `GETPARAM GRAPH_UNITS` (13) | **sí** |
| 8 | `CHANNEL_ALLOC` | sí |
| 9 | `NVIF SCLASS` + 5× `NVIF NEW` subcanal | sí (las 5 clases) |
| 10 | `NVIF DEL` ×5 + `CHANNEL_FREE` | ignorado |

Mesa **no inspecciona el errno**: solo cero/no-cero. Y con el build de Alpine
(`-Db_ndebug=true`) todos los `assert()` desaparecen, así que los fallos son
mudos.

## Bugs encontrados y corregidos

1. **Dispatch por número de request completo.** Mesa emite `VM_INIT` con
   `drmCommandWrite` (`_IOW`, `0x40106450`); el header lo define `_IOWR`
   (`0xC0106450`). Nuestro `match request` lo mandaba al brazo por defecto →
   `ENOSYS` → `has_vm_bind = false` → GPU descartada. **Este era el bug
   principal**, y hacía además que el "fix de VM_INIT standalone" de un commit
   anterior fuera *inalcanzable*. Ahora se despacha por **NR**, como Linux
   (`nouveau_drm.c`: `switch (_IOC_NR(cmd) - DRM_COMMAND_BASE)`).
2. **`NVIF` (nr 0x47) sin implementar**, y es fatal cuatro veces. Implementado
   `NEW`/`MTHD(INFO)`/`SCLASS`/`DEL`. Multiplexa cinco tamaños/direcciones sobre
   un NR, así que **sólo** funciona con dispatch por NR.
3. **`GRAPH_UNITS` devolvía `EINVAL`** y es fatal. Ahora `gpc | tpc<<8`, con la
   topología real del GSP-RM (`step15`) si el RM está atado.
4. **`PTIMER_TIME` era 10**; el header dice **14**.
5. **`CHIPSET_ID` devolvía el mínimo de la arquitectura** (0x170 = GA100/SM80,
   no GA10x/SM86). Ahora el chip id real de `NV_PMC_BOOT_0`.
6. **`VRAM_BAR_SIZE` devolvía el tamaño de VRAM** en vez de la apertura BAR1.
7. **`render_allowed` extraía el NR como `(cmd >> 8) & 0xff`** — eso es el byte
   de *tipo* (`'d'`), no el NR; aceptaba todo por accidente. Corregido.
8. **Render node en `0o660` root:root.** NVK hace `open(O_RDWR)` antes de
   cualquier ioctl y trata el fallo como "no es nouveau". Ahora `0o666`.

## El RM: corrección de una premisa (importante)

Una segunda auditoría desmontó algo que yo había afirmado antes: **el RM no se
ata sólo a mano**. `zCore/src/main.rs` ya llama a `auto_bringup_compute_gpus()`
en **cada arranque**, que corre la escalera `step5+6+8+9` y deja
`rm_device_instance` puesto — pero **sólo en las GPUs que no llevan la consola**
(a la de consola se la salta a propósito: su resume de GSP puede colgar el bus
mientras la consola pinta por su BAR1).

Y `get_primary_driver()` devuelve `drivers.first()`, que por el `insert(0)` de
`register_driver` es la **última** GPU sondeada. En esta caja de 2 GPUs eso es
la GPU de cómputo (bus 101 = 0x65) — justo la que **sí** está atada al RM.

**Conclusión: los ioctls ya iban a la GPU correcta y con RM.** El
`gpustep14` manual probablemente **no haga falta**.

### El desajuste que sí había

La **identidad sysfs** del nodo describía la **otra** GPU: `display_pci_index`
elegía el primer dispositivo clase 0x03 en orden de escaneo PCI (bus 23 = 0x17,
la de consola). libdrm alimenta a NVK con el lado sysfs (bus info de
`PCI_SLOT_NAME`, ids de `config`) mientras cada respuesta de GETPARAM/NVIF viene
del lado driver: se reportaba el id PCI de una tarjeta junto al tamaño de VRAM y
la topología de la otra. Mesa comprueba que ambos `device_id` coincidan, pero
Alpine compila con `-Db_ndebug=true` y ese `assert` desaparece — la incoherencia
era **silenciosa**.

Corregido: `display_pci_index` resuelve ahora el BDF del driver primario
(emparejando por **números**, porque el nombre del driver escribe el bus en
decimal y las rutas sysfs en hexadecimal).

`get_primary_driver()` se deja **intacto** a propósito: apuntarlo a la GPU de
consola daría `ENODEV` en cada `GEM_NEW`/`VM_BIND`/`EXEC` (no está atada) y
dirigiría los ioctls a la GPU cuyo arranque de GSP puede colgar el bus.

### El canal de descubrimiento

Se mantiene como red de seguridad: si el RM **no** estuviera atado,
`CHANNEL_ALLOC` devuelve un canal servido por software para que la GPU al menos
enumere y el fallo se vea en `GEM_NEW` con un `ENODEV` explícito, en vez de 0
GPUs sin explicación. Con el RM atado (el caso normal) esta ruta no se usa.

## Qué esperar en el próximo arranque

```sh
make iso GL=1 LOG=warn     # kernel+rootfs+ISO, con el flag y las trazas visibles
# ya arrancado:
vulkaninfo 2>&1 | head -30
dmesg | grep -E "nouveau-uapi|drm-probe|drm\] VERSION"
```

- Si NVK **enumera**, se verá la GPU en `vulkaninfo` y en `dmesg` la secuencia
  `VM_INIT → NVIF NEW → GETPARAM → NVIF MTHD INFO → GRAPH_UNITS →
  CHANNEL_ALLOC (DISCOVERY ONLY) → NVIF SCLASS`.
- Si en el log aparece `CHANNEL_ALLOC ... DISCOVERY ONLY` o `GRAPH_UNITS: RM
  not attached`, entonces el reparto consola/cómputo es el inverso de lo que
  suponemos y los ioctls van a una GPU sin RM; ahí sí haría falta
  `cat /proc/gpustep14 > /r14.txt; sync`, o preferir explícitamente una GPU no
  de consola como primaria.
- Si aún da 0 GPUs, el punto exacto de muerte estará en esa traza: es el primer
  ioctl de la tabla de arriba que no aparezca.

## Limitaciones conocidas (documentadas, no corregidas)

- **Un solo canal.** Un segundo proceso que enumere mientras otro tiene el canal
  recibe `EBUSY` y ve 0 GPUs. Mesa asigna y libera su canal de enumeración
  enseguida, así que en secuencia funciona; en paralelo no.
- **Hopper/Blackwell**: las clases de motor no están verificadas contra chip
  real. Además NVK sólo considera "conformes" `[KEPLER_A..ADA_A]` y
  `BLACKWELL_B`, así que Hopper y Blackwell-A los descarta igualmente en un
  build de release.
- **Arquitectura desconocida**: `SCLASS` rechaza en vez de adivinar una clase 3D
  (una clase equivocada haría que Mesa codifique métodos que el chip no
  implementa).
- **`GRAPH_UNITS` sin RM** reporta la configuración de die completo. Es el lado
  seguro (Mesa dimensiona el TLS de shaders con esto: pasarse sobre-asigna,
  quedarse corto peta la GPU), pero no es la verdad floorswept.
