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


## Por qué fallaba `vm_bind`: la alineación, no el tamaño del VA space

Corrección de un diagnóstico mío anterior. Con el RM ya narrando en `dmesg` se
vio:

```
NVRM: virtmemAllocResources: VA Space alloc failed! Status Code: 0x51
      Size: 0x10000  RangeLo: 0x3ffdf30000  RangeHi: 0x3ffdf3ffff
[nouveau-uapi] VM_BIND MAP failed: VA=0x3ffdf3e000 range=0x1000 -> virt_status=0x1a
```

Deduje que el VA space se quedaba corto y lo dimensioné a `1<<40`. **No era
eso**: el siguiente arranque falló exactamente igual, en las mismas dos VAs.

La causa real, seguida hasta el final en el RM vendorizado:

1. `gvaspaceApplyDefaultAlignment_IMPL` (`gpu_vaspace.c`), sin pista de
   `_PAGE_SIZE` en `attr`, entra por `RM_ATTR_PAGE_SIZE_DEFAULT` y hace
   `maxPageSize = bigPageSize` (64 KiB), luego
   `*pSize = RM_ALIGN_UP(*pSize, maxPageSize)` y
   `*pAlign = NV_MAX(*pAlign, NV_MAX(maxPageSize, compPageSize))`.
   Una petición de 4 KiB se convierte en una de **64 KiB, alineada a 64 KiB**.
2. La rama de dirección fija de `eheapAlloc` (`eheap_old.c`) es tajante:
   ```c
   if (desiredOffset % offsetAlign) goto failed;   /* -> NV_ERR_NO_MEMORY */
   ```
3. NVK asigna desde lo alto de su heap **hacia abajo en pasos de 4 KiB**, así
   que sus primeros binds caen en `0x3ffdf3e000` y `0x3ffdfae000`: alineadas a
   4 KiB, **ninguna a 64 KiB**. `0x...e000 % 0x10000 != 0` → fallo
   determinista, con `0x51` dentro y `0x1a`
   (`NV_ERR_INSUFFICIENT_RESOURCES`) hacia fuera.

Es decir: **ninguna VA que no estuviese alineada a 64 KiB podía funcionar
jamás**, con el VA space del tamaño que fuese.

La corrección es la misma que usa el propio UVM de NVIDIA
(`nvGpuOpsAllocVirtual`, `nv_gpu_ops.c`): pedir páginas de 4 KiB explícitamente
y **forzar** la alineación, que es lo que hace a
`gvaspaceApplyDefaultAlignment` saltarse el aumento — su propio comentario dice
que el cliente puede forzarla *"si se sabe que el rango de VA no se mapeará a
memoria física comprimida"*, que es justo nuestro caso (`VM_BIND` rechaza un
PTE kind distinto de cero).

Con un detalle: la **unidad** de `alignment` difiere entre las dos rutas y sólo
la física la normaliza (`memmgrAllocDetermineAlignment_GM107` hace
`(*pAlign)--  // convert to (alignment-1)`), mientras que la virtual va directa
de `pFbAllocInfo->align = pAllocParams->alignment` a
`align = pFbAllocInfo->align + 1`. La ruta VIRTUAL quiere la máscara (4095)
donde la cabecera del SDK dice "requested alignment" (4096). En vez de apostar
la función entera a esa lectura, se prueba primero la máscara y luego el
tamaño: una de las dos es la buena, la equivocada cuesta una llamada al RM
rechazada, y la traza dice cuál ganó.

Además, tras un `Map` correcto se comprueba que `actualVA == requestedVA` y se
rechaza el bind si no coinciden: NVK ya ha grabado esa VA en descriptores y
direcciones de shader antes de llamar a `VM_BIND`, así que un mapeo colocado en
otro sitio es peor que ninguno — se lee como éxito y peta (o corrompe) al
dibujar.

El `vaSize = 1<<40` se queda: un VA space que no cubra el mapa de NVK sería un
bug igualmente, y ahora la geometría concedida (`vaBase`/`vaSize`) se imprime
en **cada** reserva rechazada, así que los dos casos ya no se pueden volver a
confundir.

### Resultado en hardware: la reserva pasa, el `Map` no (todavía)

El arranque siguiente lo confirmó todo:

```
vm_bind_map: VA reserve @0x3ffffff000 (4096 B) 4KB-pages align=0xfff  -> 0x1f
vm_bind_map: VA reserve @0x3ffffff000 (4096 B) 4KB-pages align=0x1000 -> 0x0   hVirt=0x57
                                              (vas base=0x100000 size=0xfffff00000)
vm_bind_map: Map -> 0x37 actualVA=0x0
```

Tres cosas quedan zanjadas:

1. **La reserva de VA funciona.** `0x1f` = `NV_ERR_INVALID_ARGUMENT` para la
   máscara (4095), `0x0` = `NV_OK` para el tamaño (4096). Gana la lectura
   llana de la cabecera del SDK; el `+1` se absorbe aguas abajo. Se deja sólo
   esa, con la evidencia anotada en el código.
2. **El VA space era correcto**: `base=0x100000 size=0xfffff00000`, o sea
   `[1 MiB, 1 TiB)`, que contiene de sobra `0x3ffffff000`. Confirma que
   dimensionarlo nunca fue el problema.
3. **El fallo se movió al `Map`**: `0x37` = `NV_ERR_INVALID_OFFSET`.

`pDmaOffset` es IN/OUT y el significado de la mitad IN depende de
`NVOS46_FLAGS_DMA_OFFSET_FIXED`: con la bandera es una VA **absoluta**, sin
ella un desplazamiento **relativo** a la reserva. Se puso la bandera, que es lo
que de verdad queremos (un bind en una dirección exacta)... y el arranque
siguiente devolvió `0x37` **con las dos formas**:

```
Map OFFSET_FIXED@0x3ffffff000 -> 0x37 actualVA=0x3ffffff000
Map relative-0                -> 0x37 actualVA=0x0
```

Eso descartó el offset y señaló al **tamaño de página del mapeo**.

### La causa: `_PAGE_SIZE_DEFAULT` no significa 4 KiB

`dmaAllocMapping_GM107` elige el tamaño de página del mapeo desde las banderas
OS46:

```c
case NVOS46_FLAGS_PAGE_SIZE_DEFAULT:
case NVOS46_FLAGS_PAGE_SIZE_BOTH:
    pageSize = memdescGetPageSize(pTempMemDesc, ...);   // el de la alloc FÍSICA
case NVOS46_FLAGS_PAGE_SIZE_4KB:
    pageSize = RM_PAGE_SIZE;
```

Nuestra VRAM de GEM sale de una allocation con los valores por defecto del RM,
así que el tamaño de página de su memdesc es la **página grande (64 KiB)**.
Mapear dentro de una allocation virtual **ya existente** ejecuta después:

```c
vaLo = RM_ALIGN_DOWN(*pVaddr, pageSize);
if (((*pVaddr - vaLo) != 0) && ((*pVaddr - vaLo) != pageOffset))
    return NV_ERR_INVALID_OFFSET;
```

Con `pageSize = 64 KiB` y `*pVaddr = 0x3ffffff000` (alineada a 4 KiB, no a
64 KiB): `vaLo = 0x3fffff0000` y la diferencia es `0xf000` — ni 0 ni el
desplazamiento físico de página (0, la VRAM está alineada a página grande).
De ahí `0x37`, **idéntico** para la forma fija y la relativa, porque `*pVaddr`
es la misma dirección en ambas.

Es el mismo error conceptual que la reserva, un piso más abajo: la reserva de
VA se pidió a 4 KiB pero el **mapeo** seguía yendo a 64 KiB. Ahora se pide
`_PAGE_SIZE_4KB` también en el `Map`. Mapear con página **más pequeña** que la
granularidad física es la dirección segura: la corrección automática de
`dmaAllocMapping_GM107` (`physPageSize < pageSize`) sólo salta al revés.

**Coste**: PTEs de 4 KiB en todos los binds, donde un buffer alineado a 64 KiB
podría usar página grande. Es tamaño de tabla de páginas y alcance de TLB, no
corrección, y lo fuerza el reparto de VA de NVK, que es de grano 4 KiB. La
forma DEFAULT se conserva como fallback para que un arranque siga enseñando
los dos estados si alguna allocation rechazara 4 KiB.

## `VM_BIND` funciona. El siguiente muro: el dominio GART

Con los PTEs de 4 KiB en el `Map`, `dmesg | grep -i vm_bind` dejó de tener
fallos: sólo queda la línea de descubrimiento

```
[nouveau-uapi] first VM_BIND : request=0xc0286451 dir=3 nr=0x51 size=40
```

(`nr=0x51` − `DRM_COMMAND_BASE` = `0x11` = `DRM_NOUVEAU_VM_BIND`). El bind ya
no falla ni una vez.

El fallo se movió a `nvkmd_nouveau_mem.c:169`, que es el **único**
`VK_ERROR_OUT_OF_DEVICE_MEMORY` del fichero:

```c
struct nouveau_ws_bo *bo = nouveau_ws_bo_new_tiled(...);
if (bo == NULL)
   return vk_errorf(log_obj, VK_ERROR_OUT_OF_DEVICE_MEMORY, "%m");
```

y `nouveau_ws_bo_new_tiled_locked` sólo devuelve NULL cuando el ioctl
`DRM_NOUVEAU_GEM_NEW` falla. O sea: **nuestro `GEM_NEW` está rechazando la
petición.** (El `%m` imprime "Not a tty", que es un `errno` heredado del
propio camino de log de Mesa, no del ioctl — no hay que hacerle caso.)

La razón es una línea de `GEM_NEW` que sólo admitía VRAM:

```rust
if req.info.domain & NOUVEAU_GEM_DOMAIN_VRAM == 0 { return Err(EOPNOTSUPP); }
```

y NVK elige **exactamente un** dominio por asignación
(`nvkmd_nouveau_alloc_tiled_mem`):

```c
if (flags & NVKMD_MEM_GART)      domains |= NOUVEAU_WS_BO_GART;
else if (flags & NVKMD_MEM_VRAM) domains |= NOUVEAU_WS_BO_VRAM;
```

así que una petición de GART **no lleva el bit de VRAM en absoluto**. Sólo se
ve ahora porque hasta este arranque nunca pasábamos de `vm_bind`; en cuanto el
bind funcionó, NVK llegó a su primera asignación de memoria de host y se topó
con el rechazo.

### GART implementado

- **C** (`eclipse_rm_gem_alloc`): parámetro `bSysmem`. Con él,
  `NV01_MEMORY_SYSTEM` y `_LOCATION_PCI`, exactamente el patrón de la
  pushbuffer de `step17` — la única ruta de sysmem ya probada en este
  hardware — más `_PHYSICALITY_CONTIGUOUS`.
- **`gem_map_cpu`**: acepta `ADDR_SYSMEM` además de `ADDR_FBMEM`, y **rechaza**
  un objeto sysmem no contiguo. Publicamos **una** dirección física para todo
  el objeto; en una asignación dispersa eso sería sólo su primera página, y
  cada escritura de userspace más allá de 4 KiB caería sobre memoria ajena.
  Por eso se pide `_PHYSICALITY_CONTIGUOUS` arriba: para que ese rechazo no
  llegue a saltar nunca.
- **`GEM_NEW`**: VRAM si viene el bit de VRAM, GART si viene el de GART, y
  `EOPNOTSUPP` sólo si no viene ninguno. Devuelve en `req.info.domain` el
  dominio **realmente** usado, que es lo que Mesa lee de vuelta.
- Los diagnósticos de `GEM_NEW` pasan a `klog`: estaban en `log::warn!`, o sea
  invisibles al nivel de log real del hardware. Por eso este rechazo no dejó
  ni una línea en `dmesg`.

## `GEM_NEW` también pasa. Ahora `VK_ERROR_UNKNOWN`

`dmesg | grep -i gem` se queda igual que `vm_bind`: sólo la línea de
descubrimiento, ni un fallo.

```
[nouveau-uapi] first GEM_NEW : request=0xc0306480 dir=3 nr=0x80 size=48
```

El error pasa a `VK_ERROR_UNKNOWN` (−13). En NVK sólo hay tres sitios que lo
produzcan durante `vkCreateDevice` (`nvkmd_nouveau_ctx.c`):

| línea | causa |
|---|---|
| 146 | `DRM_NOUVEAU_EXEC` falla (y no con `-ENODEV`, que daría `DEVICE_LOST`) |
| 252 | `DRM_SYNCOBJ_WAIT` falla |
| 352 | `DRM_NOUVEAU_VM_BIND` falla |

VM_BIND está descartado por el `dmesg`. Quedan `EXEC` y `SYNCOBJ_WAIT`, y no se
puede distinguir desde el log **porque casi todo `EXEC` era mudo**.

### Dos defectos en `EXEC`

1. **El id de canal estaba clavado a 0.**

   ```rust
   if req.channel != 0 { return Err(nv::EINVAL); }   // sin una sola línea de log
   ```

   `CHANNEL_ALLOC` reparte ids «el menor libre», así que sólo el **primer**
   canal de un arranque es 0. Cualquier otro se rechazaba con un `EINVAL`
   pelado e invisible. Ahora se comprueba lo que de verdad importa: que el
   canal sea **del que llama** y esté **respaldado por el RM**, y cada rechazo
   dice cuál era y qué canales hay vivos.

   (`drm_nouveau_exec.channel` es `__u32` y `drm_nouveau_channel_alloc.channel`
   es `__s32`; esa asimetría está en el propio `nouveau_drm.h`.)

2. **Los diagnósticos eran `log::warn!`**, o sea invisibles al nivel de log del
   hardware — el mismo problema que ocultó el rechazo de GART. Todos pasan a
   `klog`, incluido el `EINVAL` mudo de la validación de pushbuffers.

Con esto, el próximo arranque distingue por sí solo entre `EXEC` y
`SYNCOBJ_WAIT`: si es `EXEC`, dirá exactamente por qué.

## El canal del RM se lo llevaba el canal de descubrimiento

Con `EXEC` ya hablando, el arranque lo dijo sin ambigüedad:

```
[nouveau-uapi] EXEC: channel=1 belongs to pid=17616 but is a
               DISCOVERY channel (no GR channel/GPFIFO behind it)
```

y lo mismo con pid 19555, 21378, 23223 — un labwc nuevo cada reintento, y
**siempre `channel=1`**. Es decir: el canal 0, el respaldado por el RM, estaba
cogido cuando llegó el canal de verdad.

La razón está en la secuencia que NVK ejecuta de verdad
(`nouveau_device.c`): `nouveau_ws_device_new` crea un contexto **de usar y
tirar** sólo para leer las clases de motor —

```c
if (nouveau_ws_context_create(device, ~0, &tmp_ctx)) goto out_err;
device->info.cls_eng3d = tmp_ctx->eng3d.cls;   /* ... */
nouveau_ws_context_destroy(tmp_ctx);
```

— y **sólo después** `vkCreateDevice` crea el real. Y labwc hace **dos**
`vkCreateDevice` en el mismo proceso: el de zink (por EGL/GLES2) y el del
renderizador Vulkan nativo de wlroots. Mientras ese primer canal siguiera
vivo, el canal bueno salía como DISCOVERY y todo `EXEC` sobre él se rechazaba.

Nuestro modelo era «el primer canal que pide se queda el RM, para siempre».
Eso es incorrecto: en hardware hay **un** canal GR, construido una vez por
arranque, y `step16`/`step17` son idempotentes — una llamada posterior
devuelve **la misma** asignación cacheada, no una nueva. Dos canales del
**mismo** proceso son dos nombres para una sola pieza de hardware, así que
respaldar los dos no cuesta nada.

Corregido: la exclusividad pasa a ser **por proceso**. Otro proceso distinto
sigue recibiendo un canal de descubrimiento — dos clientes compartiendo un
GPFIFO se pisarían las sumisiones —, pero dentro de un mismo cliente todos los
canales están respaldados.

Además, los fallos de la escalera (`step16`/`step17`, `ctxshare`, `sched`)
pasan a `klog`: estaban en `log::warn!` y habrían sido invisibles justo en el
punto donde más falta hacen.

## Toda la uAPI pasa sin un solo rechazo

El arranque siguiente al arreglo del canal deja el vocabulario completo, en el
orden exacto en que NVK lo emite, y **sin ninguna línea de fallo**:

```
[drm] VERSION on /dev/dri/card0     -> name="nouveau"; primary_driver="nvidia-gpu-101:0.0"
[drm] VERSION on /dev/dri/renderD128 -> name="nouveau"; primary_driver="nvidia-gpu-101:0.0"
first GETPARAM        request=0xc0106440 dir=3 nr=0x40 size=16
first VM_INIT         request=0x40106450 dir=1 nr=0x50 size=16
first NVIF            request=0x40486447 dir=1 nr=0x47 size=72
first GET_ZCULL_INFO  request=0x80306453 dir=2 nr=0x53 size=48
first CHANNEL_ALLOC   request=0xc0586442 dir=3 nr=0x42 size=88
first CHANNEL_FREE    request=0x40046443 dir=1 nr=0x43 size=4
first GEM_NEW         request=0xc0306480 dir=3 nr=0x80 size=48
first VM_BIND         request=0xc0286451 dir=3 nr=0x51 size=40
first EXEC            request=0xc0286452 dir=3 nr=0x52 size=40
```

Cosas que esto confirma de una vez:

- El nodo DRM lo sirve la GPU correcta: `primary_driver="nvidia-gpu-101:0.0"`,
  la de cómputo (bus 101 = 0x65), que es la atada al RM. La identidad sysfs y
  el lado driver ya coinciden.
- `VM_INIT` llega como `dir=1` (`_IOW`, `drmCommandWrite`) mientras la cabecera
  lo declara `_IOWR` — exactamente lo que obligó al despacho por NR.
- `CHANNEL_FREE` **sí** se emite (`size=4`), así que el canal de descubrimiento
  se libera; el problema era a quién se le daba el respaldo del RM, no una fuga.
- `grep -E "channel_alloc|step1"` no devuelve nada: ni un `DISCOVERY ONLY`, ni
  un fallo de escalera.
- `EXEC` ya no se rechaza.

labwc sigue reintentando, así que algo falla **después** del despacho. El
siguiente sitio donde puede morir es la sumisión real
(`eclipse_rm_exec_submit` / `..._signaled`), y sus resultados estaban otra vez
en `log::warn!` — invisibles. Pasan a `klog`, junto con una línea de éxito
**una sola vez por arranque**:

```
[nouveau-uapi] EXEC OK (first): N push(es) submitted and fence confirmed,
               M syncobj(s) signaled -- Mesa work is reaching the GPU
```

Esa línea es la prueba de que trabajo construido por Mesa llega a la GPU y la
valla vuelve. Si aparece, la tubería está entera; si en su lugar sale
`EXEC submit failed` o `EXEC (signaled) failed`, los seis estados del RM
(`lookup`/`map`/`token`/`submit`/`fenceSubmit`/`fenceWait`) dicen en cuál de
los pasos.

## La sumisión llega al RM y muere en el anillo: `NV_ERR_BUSY_RETRY`

```
EXEC (signaled) failed: lookup=0x0 map=0x0 token=0x0 submit=0x3
                        fenceSubmit=0xffffffff fenceWait=0xffffffff
                        (fence value=0x0 expected=0x8000005b)
```

Se lee de izquierda a derecha:

- `lookup=0x0`, `map=0x0`, `token=0x0` — **NV_OK las tres**. El buffer del canal
  y USERD se resuelven, se mapean por CPU, y el work-submit token se genera.
  Toda la fontanería del canal funciona.
- `submit=0x3` = **`NV_ERR_BUSY_RETRY`**: el anillo GPFIFO se ve lleno.
- `fenceSubmit`/`fenceWait = 0xffffffff` es el centinela «ni se intentó»: se
  aborta antes.

Lo importante: **abortamos antes de tocar `GPPut`**, así que estos fallos no
pueden ser los que llenan el anillo. O lo llenaron sumisiones anteriores que
sí escribieron, o `GPGet` no avanza y `used` crece sin volver a bajar.

La aritmética encaja con la segunda: el camino con valla consume **2** ranuras
por llamada (la del cliente + la de la valla del kernel) y el anillo tiene 128
(`ECLIPSE_CHAN_GPFIFO_ENTRIES`), o sea 63 sumisiones antes de que
`used + 2 > entries - 1`. El contador de valla de las líneas visibles va por
`0x8000005b` = la 91ª — coherente con «las primeras ~63 escribieron, el resto
rebota».

Falta un dato para decidirlo, y es el que faltaba: los números crudos. Ahora
las dos ramas de `BUSY_RETRY` los imprimen:

```
[eclipse-rm-trace] exec_submit_signaled: RING FULL GPPut=%u GPGet=%u
                   (put=%u get=%u used=%u entries=%u, need 2 slots)
```

- `GPGet` congelado en un valor mientras `GPPut` se ha ido → el canal **no
  consume** el anillo (no está planificado, o el timbre no suena, o la VA del
  pushbuffer no es válida para el host).
- `GPGet` moviéndose y `GPPut` por delante → el canal simplemente va por
  detrás, y lo que falta es esperar hueco en vez de rechazar.

Además, `EXEC` pasa a **capturar y reproducir la narración del RM** por `klog`
cuando falla, igual que ya hacía `VM_BIND`: sin eso estas líneas dependían del
sumidero por defecto y del nivel de log.

### El anillo: `GPPut=0 GPGet=1`, el canal se paró tras UNA entrada

```
exec_submit_signaled: RING FULL GPPut=0 GPGet=1
                      (put=0 get=1 used=127 entries=128, need 2 slots)
```

`used = (put + entries - get) % entries = (0 + 128 - 1) % 128 = 127`.

Comprobado que la estructura de USERD es la vendorizada (`Nvc46fControl` en
`clc46f.h`: `GPGet` en 0x88, `GPPut` en 0x8c, en ese orden), así que los
offsets son correctos y el estado es real, no un desajuste de campos.

Y el estado dice esto: el productor dio la vuelta entera al anillo mientras el
consumidor avanzó **exactamente una** entrada. `GPGet=1` y ahí se quedó. Las
primeras ~63 sumisiones sí escribieron (2 ranuras cada una: la del cliente más
la de la valla), llenaron las 127 y desde entonces todo rebota con
`NV_ERR_BUSY_RETRY`.

Es decir: **la uAPI entera está bien y el canal no ejecuta**. El host FIFO
consumió una entrada y se detuvo — la firma de un canal al que la recuperación
robusta (robust channel) tumbó: fallo de MMU, error de PBDMA o excepción de
GR.

`step17` pasa `hObjectError = hNotifier` en `NV_CHANNEL_ALLOC_PARAMS`, así que
el RM escribe una `NvNotification` en ese buffer justo cuando eso ocurre. Es
lo único a mano que dice **cuál** de los tres fue. Ahora se vuelca en la rama
de anillo lleno:

```
[eclipse-rm-trace] chan error notifier: status=0x.. info32=0x.. info16=0x..
```

`status != 0` significa que la recuperación robusta saltó; `info32` es el
código de error RC.

La línea de `RING FULL` pasa además a imprimirse **una vez por estado distinto
de `(put, get)`**: un canal atascado hace que todo EXEC posterior caiga ahí, y
sin estrangular sepultaba el volcado del notificador bajo cientos de filas
idénticas.

### Regresión mía y corrección: el volcado del notificador tumbó el kernel

El volcado que añadí en la rama de anillo lleno mapeaba el notificador con
`memmgrMemDescBeginTransfer` y lo leía ahí mismo. En hardware, esa superficie
de transferencia devolvió una VA de kernel (`0xffff8010_8168_8000`) que las
tablas de páginas de Eclipse **no cubren**, y la lectura tiró el kernel
entero:

```
[KERNEL PAGE FAULT] vaddr=0xffff801081688000 flags=READ err=NOT_FOUND
[PANIC] cpu=4 at zCore/src/handler.rs:215:21
```

Ese camino está eliminado. El reemplazo lee el notificador **a nuestra
manera**, con dos piezas ya probadas en esta máquina:

1. El RM sólo entrega la **dirección física** de la página
   (`eclipse_rm_chan_notifier_pa`): bookkeeping puro de memdesc, la misma
   receta que `gem_map_cpu` ejecuta en cada `GEM_NEW`.
2. El lado Rust la mapea con `crate::bus::phys_to_virt` — la misma ventana que
   usa el blit del framebuffer — y lee los 16 bytes de la `NvNotification`
   (`info32` en +8, `info16` en +12, `status` en +14).

Se vuelca **una vez por arranque**, sólo cuando `submit_status ==
NV_ERR_BUSY_RETRY` (el anillo atascado):

```
[nouveau-uapi] chan error notifier @PA 0x..: status=0x.. info32=0x.. info16=0x..
```

`status != 0` → la recuperación robusta saltó; `info32` es el código RC que
dice si fue fallo de MMU, error de PBDMA o excepción de GR.

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

## `GL=1` en QEMU: la bandera es una petición, no una capacidad

`GL=1` estampa `renderer=gl:nvidia.nouveau_uapi` en **una sola** línea de
comandos, y esa imagen se arranca tanto en la caja RTX como en QEMU. El
kernel activaba la uAPI nouveau con sólo ver la bandera, así que **en QEMU
también** quedaba activa, sin ninguna GPU NVIDIA presente. Eso cambiaba, en
la ruta del escritorio por software que sí funciona:

- `DRM_IOCTL_VERSION` respondía `name="nouveau"` versión 1.4.0 sobre el nodo
  de virtio-gpu → Mesa carga **NVK** y dirige toda la uAPI nouveau a un
  driver que no implementa ni uno de esos ioctls.
- `DRM_CAP_SYNCOBJ` / `_SYNCOBJ_TIMELINE` pasaban a anunciarse como 1, así
  que wlroots empieza a usar syncobjs donde antes no los usaba.
- Se abrían los siete ioctls `DRM_IOCTL_SYNCOBJ_*` y `sys_drm_syncobj_fd`.
- El klog de `VERSION` (sin límite de repetición, ver abajo) se emitía por
  cada `drmGetVersion`.

Ahora hay dos condiciones: la bandera **y** que algún driver DRM registrado
declare `DrmScheme::nouveau_uapi_capable()`, que sólo `NvidiaGpu` hace. Sin
GPU NVIDIA el kernel lo dice y sigue identificándose como `"zcore"`, igual
que antes de que `GL=1` arrastrase la bandera. En la RTX no cambia nada.

### Klogs sin estrangular

`klog_info!` no tiene filtro de nivel, ni límite de frecuencia, y escribe de
forma síncrona por el UART. En rutas cuya cadencia controla el espacio de
usuario eso es un problema por sí solo: `VERSION` se emite en cada
`drmGetVersion`, o sea en cada `drmGetDevices2`, cada sondeo de Vulkan/EGL y
cada reintento del backend de wlroots. Ahora se registra **una vez por nodo**
(el mismo patrón de `AtomicBool` que ya tenía la línea de `render_allowed`);
la respuesta que lleva es idéntica cada vez, así que la primera es todo el
diagnóstico.
