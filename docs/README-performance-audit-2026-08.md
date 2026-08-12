# Auditoría de rendimiento de Eclipse OS — agosto 2026

Revisión sistemática de todo el sistema operativo para responder a una sola
pregunta: **¿por qué el sistema se siente lento?** Se auditaron el planificador
(`vendor/PreemptiveScheduler`), el HAL (`kernel-hal`), los drivers
(`drivers/`), la pila de red (`linux-object/src/net`, `smoltcp`), la ruta de
syscalls (`linux-syscall`, `linux-object`, `zircon-object`), la consola y la
pila gráfica, el asignador de memoria (`zCore/src/memory_x86_64.rs`) y las
herramientas de usuario (`lunarbg`, `lunarbar`).

Conclusión general: el núcleo del planificador, el timer y el asignador ya
recibieron mucho trabajo de optimización (timer por deadline, preempción por
wakeup, slab frontal, lazy-TLB, caché de bloques con read-ahead) y están en
buen estado. La lentitud percibida NO viene de ahí: viene de **(1) la ruta
gráfica/consola, (2) los drivers de almacenamiento, y (3) esperas activas
(busy-wait) en la pila de red**, más una colección de costes fijos de
diagnóstico que se pagan en cada asignación/IRQ/syscall.

---

## Resumen ejecutivo — las 12 causas principales

| # | Causa | Zona | Impacto |
|---|-------|------|---------|
| 1 | El **demand paging de archivos está anulado**: `map_range \|\| vmo.name() != ""` fuerza el commit eager de TODO archivo mapeado (una lib de 30 MiB = ~7 700 lecturas síncronas de 4 KiB dentro del `mmap`, con locks IRQ-off) | `zircon-object/src/vm/vmar.rs:489-502` | CRÍTICO |
| 2 | `poll`/`select`/`epoll` **no bloquean**: sondean cada 4 ms (250 despertares/s por proceso; cada IPC Wayland/X11/D-Bus queda cuantizado a 4 ms; las CPUs nunca llegan a `hlt`) | `linux-syscall/file/poll.rs`, `linux-object/net/wait.rs` | CRÍTICO |
| 3 | El framebuffer se mapea **sin write-combining** (UC puro): cada store es una transacción PCIe serializada | `kernel-hal/vm.rs` + `rboot` | CRÍTICO |
| 4 | Cada línea de consola marca sucia **toda la pantalla** → blit completo (~8 MB) por línea impresa | `graphic_console.rs` | CRÍTICO |
| 5 | AHCI 100 % por polling, con **IRQs deshabilitadas durante todo el comando** y bajo mutex de puerto | `drivers/src/ata/ahci.rs` | CRÍTICO |
| 6 | El resolver DNS hace espera activa (spin) hasta **9,6 s a 100 % de CPU** por lookup fallido | `linux-object/src/net/dns.rs` | CRÍTICO |
| 7 | virtio-blk emite **una petición por sector de 512 B** (128 round-trips por lectura de 64 KiB) | `drivers/src/virtio/blk.rs` | CRÍTICO |
| 8 | Page-flip de DRM copia **el frame completo por CPU con IRQs apagadas** (varios ms de latencia de interrupción por frame) | `linux-object/fs/devfs/drm.rs` | CRÍTICO |
| 9 | `sys_write` ejecuta un escaneo de subcadenas de 12 agujas (~6 100 comparaciones) en **cada write(2)** de cualquier fd | `linux-syscall/file/file.rs:1414` | ALTO |
| 10 | `write()` TCP bloqueante gira sin ceder CPU hasta 30 s; ARP/NDP giran 25 ms×4 asignando 2 KiB por iteración; la capa FS pide bloques de **512 B** a los drivers | `linux-object/src/net`, `rcore_fs_wrapper.rs` | ALTO |
| 11 | Se crean **64 executors al arrancar** aunque haya 4 CPUs: ~168 MiB de heap perdidos (33 % del heap de 512 MiB) + envenenado `write_volatile` de 2 MiB por executor | `vendor/PreemptiveScheduler` | ALTO |
| 12 | Sin fault-around (1 página por fallo), pipes byte a byte, lookups de ruta sin caché que re-recorren el CWD desde la raíz, UART TX con spin por byte | varios | ALTO |

---

## 1. Pila gráfica y consola (la lentitud "visible")

### 1.1 CRÍTICO — No existe write-combining: el framebuffer es memoria UC

`kernel-hal/src/bare/arch/x86_64/vm.rs:126-134`: `CachePolicy::WriteCombining`
emite exactamente los mismos bits (`PCD|PWT`) que `Uncached`; el bit PAT no se
pone y **nada en todo el árbol programa el MSR `IA32_PAT` (0x277) ni las
MTRRs** (grep en `kernel-hal`, `rboot`, `drivers`, `zCore/src`: cero
resultados). Además el framebuffer GOP se alcanza por el alias del physmap
(`drivers.rs:138`, `phys_to_virt(fb_addr)`), que `rboot` mapea solo con
`PRESENT|WRITABLE` (`rboot/src/page_table.rs:174`), sin atributo de caché
especial.

Consecuencia: todos los `copy_from_slice` del blit final
(`drivers/src/scheme/display.rs:344`) aterrizan en memoria **uncacheable**:
cada store de 8 B es una transacción serializada. Es la diferencia entre
~50-100 MB/s y varios GB/s de ancho de banda de blit. Todo el diseño del
`ShadowFramebuffer` (correcto: componer en RAM cacheada y volcar una vez) queda
anulado en el último paso.

**Arreglo**: programar PAT (una entrada WC), mapear la apertura del
framebuffer con WC, y usar stores `movnti`/`rep movsb` para el volcado.
Cambio pequeño y la mayor ganancia visible de toda la auditoría.

### 1.2 CRÍTICO — Blit de pantalla completa por cada línea de consola

`drivers/src/utils/graphic_console.rs:281-298`: el scroll de una línea usa
`shadow.copy_rect(0, CHAR_HEIGHT, 0, 0, width, text_h-CHAR_HEIGHT)`, y
`copy_rect` marca sucio el rectángulo destino → **la caja sucia pasa a ser la
pantalla entera**. Encadenado con:

- `linux-object/src/fs/stdio.rs:1501-1516` — cada `write(2)` se parte en cada
  `\n`, con una llamada a consola por trozo;
- `kernel-hal/src/common/console.rs:177-184` — se hace `present()` después de
  **cada** llamada.

Resultado: a 1920×1080 ARGB, ~8,3 MB de memmove en RAM **+** ~8,3 MB de copia
a la apertura (que además es UC, ver 1.1) **por cada línea impresa**. Un
`cat` de 1000 líneas mueve varios GB. Esto por sí solo explica "la consola va
lenta".

**Arreglo**: (a) hacer el scroll con `display.copy_rect` (ya implementado en
`drivers/src/scheme/display.rs:266-300` y con `accel_caps()` anunciado por
ambos backends, pero **nunca llamado por nadie** — código muerto), marcando
sucia solo la fila nueva; o (b) dirty-rects por fila en vez de una única caja
englobante (`shadow_fb.rs:80-101`).

### 1.3 CRÍTICO — Page-flip DRM: copia CPU del frame completo con IRQs off

`linux-object/src/fs/devfs/drm.rs:552-562`: en cada flip se hace
`intr_off(); display.blit_from(0,0,pixels,stride,width,height); intr_on()`
con `width×height` = modo completo. `DRM_IOCTL_MODE_DIRTYFB` no está
implementado, así que el compositor no puede recortar el daño. Una copia de
~8 MB a 60 Hz son ~500 MB/s solo de copia, y la ventana con IRQs apagadas dura
varios milisegundos → picos de latencia de entrada en cada frame (es la causa
directa del aviso "your system is too slow" de libinput que menciona
`tools/lunarbg/src/main.rs:751-755`).

**Arreglo**: implementar `DIRTYFB` y copiar solo rects dañados; trocear la
copia para no mantener IRQs apagadas; con 1.1 arreglado, la copia además se
vuelve ~10× más barata.

### 1.4 ALTO — Redibujado de TUI sin comparación de celdas

`graphic_console.rs:229-248` (y `vendor/rcore-console/src/text_buffer_cache.rs:51-56`):
`write(row, col, cell)` re-rasteriza el glifo aunque `self.buf[row][col] ==
cell` (el valor anterior está ahí y `Cell` deriva `PartialEq`, pero no se
compara). htop/vim repintan la pantalla completa por refresco con >90 % de
celdas idénticas. Un guard de una línea da un orden de magnitud en TUIs.

### 1.5 ALTO — Dibujo píxel a píxel por `draw_iter`

`graphic_console.rs:38-55`: `ShadowDraw` solo implementa `draw_iter`, de modo
que `fill_solid`/`fill_contiguous`/`clear` de embedded-graphics degeneran en
iteradores por píxel con bounds-check (162 iteraciones por glifo 9×18; los
bordes y barras de htop se pintan píxel a píxel). Además `put_pixels` llama a
`mark()` **por píxel** (`shadow_fb.rs:91-101`) en vez de una vez por glifo.

### 1.6 ALTO — Scroll de consola con alocaciones por fila

`graphic_console.rs:257-274`: cada newline clona una `Vec<Cell>` por fila
(~114 alocaciones y ~410 KB copiados por línea a 227×113) en vez de
`rotate_left(1)`/`VecDeque`. Igual en `scroll_region_up/down` y `clear()`.

### 1.7 ALTO — Consola temprana (boot): stores volátiles por píxel

`kernel-hal/src/bare/arch/x86_64/early_fb_console.rs:85-153`: `put_pixel` hace
5 cargas atómicas SeqCst + 1 store volátil por píxel; el "scroll" es
`clear_black()` = ~2 M de stores volátiles individuales a memoria GOP (que
además es UC). Cada vez que el log de arranque llena la pantalla se pagan
cientos de ms. Impacto directo en el tiempo de arranque percibido.

### 1.8 MEDIO — virtio-gpu: flush de pantalla completa e ignorando dirty rect

`drivers/src/virtio/gpu.rs:120-129` + `scheme/display.rs:456-459`:
`DisplayScheme::flush()` no recibe rectángulo, y el driver virtio hace
`transfer_to_host_2d` de la resolución completa en cada present — incluido el
**parpadeo del cursor a 2 Hz en idle**. (No afecta a la ruta UEFI/GOP por
defecto en x86_64, sí a virtio-gpu y riscv/aarch64.)

### 1.9 Limpio — lunarbg / lunarbar

Las herramientas de usuario están bien: `poll()` con timeout correcto, tope de
24 fps, daño limitado a la región del logo, sin espera activa. Solo detalles
menores (lunarbar daña la barra completa 1×/s y reasigna un `Pixmap` de
~300 KB por repintado).

---

## 2. Almacenamiento (drivers + capa FS)

### 2.1 CRÍTICO — AHCI: polling puro con IRQs deshabilitadas todo el comando

`drivers/src/ata/ahci.rs`:
- L382 `write_reg(PORT_IE, 0)` y L942: las interrupciones AHCI se deshabilitan
  a propósito; `handle_irq` (L1258) está vacío.
- `wait_until` (L209-235) es un spin crudo sin `hlt` ni yield.
- El mutex del puerto se toma **antes** del bucle y se mantiene durante todo el
  comando, reintentos incluidos (L1171, L1211); `lock::Mutex` hace
  `intr_off()` → **todo el I/O de disco transcurre con interrupciones
  apagadas en esa CPU**: ni tick, ni teclado, ni red. Peor caso:
  `CMD_TIMEOUT_US=10 s × RW_RETRIES=8` ≈ 80 s de spin con IRQs off ante un
  sector malo.
- `rw_block` usa siempre el slot 0 (L502): los 32 slots y NCQ (detectado y
  logueado en L950-957) no se usan — profundidad de cola 1.
- El parche paliativo `net_drain_throttled()` (L255-263, L450) **re-entra en
  toda la pila smoltcp desde dentro del spin del disco** cada 300 µs,
  manteniendo el lock AHCI: amplificación de latencia sin límite y riesgo de
  orden de locks.

### 2.2 CRÍTICO — AHCI: la ruta zero-copy está desactivada con `&& false`

`ahci.rs:1184` y `:1220`: `if ptr.is_multiple_of(4096) && false` — todo pasa
por el bounce buffer de un solo PRD con `copy_nonoverlapping` extra, y
`clflush_range` se ejecuta **tres veces por lectura** (antes, al completar y en
el readback de PRDBC). El PRDT de 56 entradas no se usa nunca.

### 2.3 CRÍTICO — virtio-blk: una petición virtio por sector de 512 B

`drivers/src/virtio/blk.rs:49-75`: `read_block` itera `chunks_mut(512)` y por
cada trozo construye cadena de descriptores + doorbell + espera síncrona, bajo
un mutex que apaga IRQs. Una lectura de 64 KiB = **128 round-trips
serializados**. Es la ruta por defecto de `make qemu` con DISK=on.

### 2.4 CRÍTICO — NVMe: QD1, interrupciones enmascaradas, clflush en el spin

`drivers/src/nvme/interface.rs`: INTMS=0xffffffff (L276), `handle_irq` vacío,
`submit_sync` gira sobre el bit de fase **haciendo `clflush` del CQE en cada
iteración** (L148, instrucción serializante en el bucle más caliente), bounce
de 8 KiB por comando (`nvme_queue.rs:47`), y el lock de la cola se mantiene
durante todo el bucle multi-chunk (L458/L496).

### 2.5 ALTO — La capa FS pide 512 B: multiplicador de todo lo anterior

`linux-object/src/fs/rcore_fs_wrapper.rs:53-57` (`BLOCK_SIZE_LOG2 = 9`) hace
que rcore-fs llame al driver **una vez por bloque de 512 B**; la montura FAT
(`fat_mount.rs:52-61`) lee sector a sector a un buffer temporal sin caché
alguna. Una página de 4 KiB = 8 comandos completos de disco (con su lock, su
ventana IRQ-off, su bounce y su clflush cada uno). La ruta btrfs/ext2 vía
`BlockByteDevice`/`CachedDevice` sí agrupa y cachea correctamente (dos niveles
de caché LRU + read-ahead de 1 MiB con detección de streams — bien diseñada).

### 2.6 Nota — el rootfs por defecto (initramfs SFS en RAM) no paga esto

El SFS raíz monta sobre `MemBuf` (RAM), así que el arranque por defecto no
sufre 2.1-2.4; los sufren los discos reales (`/dev/vda`, AHCI, NVMe) y
cualquier montura FAT/ext2/btrfs.

---

## 3. Red

### 3.1 CRÍTICO — DNS: hasta 9,6 s de espera activa por lookup

`linux-object/src/net/dns.rs:352-357` (`spin_ms`): espera activa pura con
`spin_loop()`, sin `await` ni timer. Se llama desde `udp_exchange` (L411)
dentro de `for round in 0..32` (L389) → una query sin respuesta = 32×50 ms =
**1,6 s de CPU al 100 %**. `resolve()` (L91-104) lanza A **y** AAAA por cada
nameserver y solo corta después de ambas: con 3 nameservers y `Unspec`
(lo que usa `getaddrinfo`) = **~9,6 s de core clavado** por un lookup fallido;
incluso un lookup con éxito paga típicamente el spin completo de la AAAA.
Cualquier aplicación que resuelva nombres (apk, wget, curl) congela la
máquina.

### 3.2 CRÍTICO — `write()` TCP bloqueante: spin síncrono de hasta 30 s

`linux-object/src/net/tcp.rs:320-343`: con el buffer TX lleno, el bucle gira a
toda velocidad (llamando `poll_ifaces()` completo por iteración) hasta 30 s.
`read`/`connect`/`peek` ya se portaron a `NetRxOrTimeoutFuture` (aparcan bien);
`write` sigue siendo `fn` síncrona sin punto de parqueo.

### 3.3 ALTO — ARP/NDP: spin de 25 ms×4 con 2 KiB de alloc por iteración

`linux-object/src/net/mod.rs:1291-1300` y `:1488-1497`: bucles de espera
activa que además llaman `drain_ipv4_nic(dev, 2)` por iteración, y
`netdev_drain_rx` (L1007) **asigna `vec![0u8; 2048]` en cada llamada** →
decenas de miles de pares alloc/free de 2 KiB en 25 ms. Tormenta de presión
sobre el asignador global.

### 3.4 ALTO — Cada `write()` TCP con éxito ejecuta 128 rondas de polling

`mod.rs:1091-1093` (`flush_socket_egress` → `drain_net_poll(128)`): las rondas
2-128 casi siempre son inútiles (el poll real está throttled a 4-32 µs) pero
cada una paga ~4 `JOBS.lock()` con cli/sti + `rdtsc`: **~500 operaciones de
lock que alternan interrupciones por syscall de escritura**.

### 3.5 MEDIO — `get_net_device()` asigna y ordena en cada poll

`kernel-hal/src/bare/net.rs:66-71`: clona el `Vec` de dispositivos, y
`get_ifname()` devuelve `String` (clon por dispositivo) en cada llamada;
lo llaman `poll_ifaces`, `has_usable_ipv4`, `prepare_ipv4_stack`,
`select_ipv4_for_dst`… varias veces por ronda de drain.

### 3.6 MEDIO — `poll_delay()` de smoltcp no se usa; wakes agregados y caídos

- El fork vendorizado de smoltcp es 0.8.0 estándar sin modificaciones; sus
  `poll_at()/poll_delay()` no tienen ni un solo call-site: el stack se sondea
  con constantes fijas (4/16/32 ms en `adaptive.rs`, fallback de 5 ms por
  socket), así que los timers TCP se sirven tarde o de más.
- `wake_net_rx_waiters` (`kernel-hal/src/bare/net.rs:125-136`): lista única
  global de wakers (thundering herd: cada paquete despierta a TODOS los
  sockets bloqueados, y cada uno hace un `poll_ifaces()` completo), y el
  rate-limit de 1 ms **descarta** el wake en vez de aplazarlo → hasta 5 ms de
  latencia añadida por RX bajo carga.

### 3.7 MEDIO — Datapath NIC con 3 alocaciones+copias por paquete

`e1000e.rs:1410/1412/2162`, `net/mod.rs:222-228`: `to_vec()` por frame RX,
otra copia al diferirlo (bajo mutex global), `vec![0;len]` por frame TX, y un
spin de TX (`TxToken::consume`, L2178-2191) manteniendo el lock del hardware
con IRQs off — bloqueando la misma interrupción que liberaría el slot.

### 3.8 MEDIO — Cola de trabajos diferidos: drena 2 por pasada y descarta

`drivers/src/utils/deferred_job.rs:18-41`: `MAX_JOBS_PER_DRAIN=2` y con la
cola llena se desaloja el más viejo; tan real es la pérdida que `e1000e.rs`
tiene `heal_stuck_poll_pending()` para "resucitar" NICs que quedaron sordas.

---

## 4. Planificador, memoria y costes fijos del kernel

### 4.1 ALTO — 64 executors creados al arrancar aunque haya 4 CPUs

`vendor/PreemptiveScheduler/src/runtime.rs:416-427`: `GLOBAL_RUNTIME` es
`[Mutex<ExecutorRuntime>; MAX_CORE_NUM]` con `MAX_CORE_NUM=64`
(`kernel-hal/src/config.rs`), construido entero por `warm_runtimes()` en el
boot (`kernel-hal/src/bare/boot.rs:45`). Cada `Executor::new`
(`executor.rs:386-472`):

- asigna 2,625 MiB de pila (2 MiB usable + 512 KiB guard + 64 KiB top-guard);
- **envenena los 2 MiB con `write_volatile` de 8 B** (262 144 stores que el
  compilador no puede vectorizar);
- instala guardas duras desmapeando 144 páginas (con sus invalidaciones TLB).

Con `SMP=4` (el default de QEMU), 60 de los 64 son puro desperdicio:

- **~168 MiB de heap del kernel consumidos para siempre — el 33 % del heap
  de 512 MiB** (el mismo heap cuyo OOM en sesión de escritorio obligó a
  subirlo de 256 a 512 MiB; la mitad de esa subida la comen estas pilas);
- ~134 MiB de stores volátiles + ~9 200 desmapeos de página en serie en la
  CPU de arranque → arranque más lento.

**Arreglo**: construir los runtimes perezosamente por CPU presente
(`cpu_count()` real), o al menos el envenenado solo bajo una feature de
diagnóstico. Nota: el envenenado de 2 MiB también se paga en cada executor
nuevo en runtime (downgrade tras abandono/oops), aunque hoy eso es raro.

### 4.2 ALTO — Sin páginas globales ni PCID: cada cambio de proceso vacía TODO el TLB

`kernel-hal/src/bare/arch/x86_64/vm.rs:82-103` (`pt_clone_kernel_space`): el
bit G se quitó (correctamente, era un bug en AMD ponerlo en la PML4E), pero no
se reintrodujo en las PTE/PDE hoja, y no hay PCID (`Cr3Flags::empty()`).
Cada escritura de CR3 entre procesos distintos invalida también las entradas
del kernel. El lazy-TLB existente (`thread.rs:1219-1231`,
`activate_kernel_paging` solo en idle) evita el flush en polls consecutivos
del mismo proceso, pero cualquier carga multi-proceso (pipelines de shell,
compositor+clientes) paga recarga completa de TLB por cambio. El propio
comentario del código lo deja como TODO.

### 4.3 MEDIO — Diagnósticos permanentes en rutas calientes

Costes fijos que hoy se pagan siempre (razonables mientras se caza la
corrupción de heap descrita en `docs/README-crash-repro.md`, pero conviene
saber que están):

- **Cada alloc del heap del kernel**: `alloc_overlaps_live_stack` escanea 256
  slots atómicos (`executor.rs:177-191`, llamado desde
  `memory_x86_64.rs:735`); `hot_track` escanea hasta 32 slots más, dos
  histogramas atómicos (alloc y dealloc).
- **Cada `frame_alloc`**: `overlapping_live_stack` escanea 128 slots
  (`zCore/src/memory.rs:118-128`).
- **Cada IRQ** (timer 250 Hz × N CPUs + dispositivos):
  `check_current_executor_canary` + `check_current_executor_stack_proximity`
  (try_lock del runtime + lecturas de canary) (`trap.rs:705-708`), más
  `irq_should_skip_dyn_dispatch` (2-3 try_locks) en cada tick
  (`timer.rs:481-484`).
- **Cada syscall**: `hunter::check_syscall` (bien optimizado: fast-path sin
  lock salvo anomaly-detection activa, shard-lock por pid si lo está),
  `check_ext_intact` ×2 (barato), contabilidad perf (2× `timer_now`).
- `TaskCollection` crea 32 `FutureCollection` por CPU pero solo se usa la
  prioridad 4; `has_ready()` recorre las 32 con try_lock en cada pre-halt.

Ninguno de estos es "el" problema, pero suman un impuesto de fondo de un
poco de % en cargas intensivas de alloc/syscall, y son los primeros candidatos
a poner detrás de features cuando se cierre la caza de la corrupción.

### 4.4 MEDIO — FXSAVE/FXRSTOR incondicional en cada entrada/salida a usuario

`vendor/trapframe/src/arch/x86_64/syscall.rs:74-88`: cada syscall/trap paga
`fxrstor` + `fxsave` (~150-200 ciclos). Linux lo evita dejando el estado FPU
en registros entre syscalls del mismo hilo. Nota adicional de corrección:
`fxsave` no guarda YMM/ZMM — con `-cpu Haswell` (AVX2 visible para el guest)
dos hilos que usen AVX pueden corromperse mutuamente la mitad alta de los
registros; convendría `xsave`/`xsaveopt` o esconder AVX en CPUID.

### 4.5 INFO — El corazón del planificador está bien

Para contexto: el executor duerme con `hlt` correctamente (protocolo
sleeping-mask + IPI de reschedule contra lost-wakes), hay preempción por
wakeup con coalescing de IPIs, robo de trabajo con try_lock sin spins, timer
por deadline (LAPIC re-armado a la próxima expiración), fast-path sin lock en
`timer_tick`, y el asignador tiene slab frontal O(1) sobre el buddy. La caché
de bloques (`block_mount.rs`) con read-ahead por streams está bien diseñada.
No se encontró el clásico "bucle de polling de red a 100 % de CPU" (ya fue
eliminado; los busy-waits que quedan son los puntuales de §3).

---

## 5. Ruta de syscalls, VM y objetos

### 5.1 CRÍTICO — El demand paging de archivos está desactivado por accidente

`zircon-object/src/vm/vmar.rs:489-502`:

```rust
// TODO: Fix map_range bugs and remove this line
let map_range = map_range || vmo.name() != "";
```

`sys_mmap` pasa deliberadamente `map_range=false` para mapeos de archivo
(`linux-syscall/src/vm.rs:332,349,371`, con un comentario que explica que el
commit eager "re-introduciría la lectura completa del archivo que congelaba la
máquina"). Pero **todos los VMO respaldados por archivo llevan nombre**
(`linux-object/src/fs/file.rs:805,830,313` — `vmo.set_name(&path)`), así que
la condición `vmo.name() != ""` fuerza `map_range=true` para
**cada biblioteca y ejecutable**.

`VmMapping::map()` (`vmar.rs:1728-1746`) recorre entonces cada página llamando
`commit()`, y cada fallo va a `FileFrameFiller::fill_page`
(`file.rs:392-408`) que emite **una lectura `read_at` de 4 KiB**. Mapear una
biblioteca de 30 MiB = ~7 700 lecturas síncronas de 4 KiB + 7 700 frame
allocs, todo dentro del `mmap()`, y con tres locks en mano (family lock del
VMO por todo el rango, `inner.lock()` — un spinlock que apaga IRQs, ver la
advertencia del propio código en `vmar.rs:1149` — y `page_table.lock()`).

Esto explica los `execve`/`dlopen` de varios segundos y el "congelado al
arrancar" de cualquier aplicación dinámica de escritorio. Coste secundario:
`vmo.name()` toma un mutex y **clona un String** por cada `map_ext_min` solo
para comparar contra `""`.

**Arreglo (una línea)**: quitar `|| vmo.name() != ""` (o condicionarlo a algo
que no sea "es un mapeo de archivo").

### 5.2 CRÍTICO — poll/select/epoll sondean cada 4 ms en vez de bloquear

`linux-object/src/net/wait.rs:13` (`IO_WAIT_TICK_MS = 4`) +
`linux-syscall/src/file/poll.rs:226-250` / `:522-527` +
`linux-object/src/fs/epoll.rs:262-343`: un `poll(-1)` no aparca en los wakers
de los fds vigilados; arma un timer de 4 ms y repite el escaneo completo de
readiness. Consecuencias:

- **250 despertares/s por proceso bloqueado.** Un escritorio con 40 procesos
  en `epoll_wait` son 10 000 despertares/s del executor sin hacer nada — las
  CPUs no llegan a `hlt` (exactamente la firma "busy while idle" que
  `/proc/perf/kernel` está diseñado para diagnosticar).
- **Hasta 4 ms de latencia añadida en cada evento de I/O** no cubierto por un
  IRQ de red/HID — en particular pipes y unix sockets: **cada round-trip
  X11/Wayland/D-Bus queda cuantizado a 4 ms**. Esto es lentitud de escritorio
  pura.
- Cada tick asigna: `Epoll::wait` reconstruye su `Vec` con un bump de `Arc`
  por fd vigilado, 250×/s; y `select_core` (`poll.rs:427`) **clona el HashMap
  completo de fds del proceso** (`process.rs:1081-1084`) 250 veces por
  segundo.

### 5.3 CRÍTICO — Sin fault-around: 1 página = 1 trap + 1 lectura de 4 KiB

`vmar.rs:2065`: `handle_page_fault` comitea exactamente una página (el
comentario de `vmar.rs:1151` menciona un fault-around de 16 páginas que **no
existe en el árbol**). Cada 4 KiB de un archivo paginado por demanda cuesta un
trap completo + locks + lectura de 4 KiB + invalidación TLB. Linux usa 64 KiB
(16 páginas) por defecto.

### 5.4 ALTO — `sys_write` escanea 12 subcadenas en cada write

`linux-syscall/src/file/file.rs:119-122` + `:1414-1440` (`tee_x_diag`): hasta
**12 × 512 ≈ 6 100 comparaciones de ventana deslizante por cada `write(2)`**,
incondicionalmente, en cualquier fd (pipes, sockets, archivos). Es un
diagnóstico de arranque de Xorg instalado permanentemente en la syscall más
caliente del sistema. `sys_writev` igual con 1024 B.

### 5.5 ALTO — Impuesto fijo por syscall

Cada syscall paga, incondicionalmente (`lib.rs:200-234, 622-628`):

- `hunter::check_syscall` → `heuristics::on_syscall` con **mutex sharded
  IRQ-off + BTreeMap `entry()`**: el fast-path sin lock exige `!anomaly`,
  pero `ANOMALY_ENABLED` **arranca en `true`** (`heuristics.rs:38`), así que
  nunca se toma; 16 shards por `pid % 16` → todos los hilos de un proceso
  contienden el mismo lock.
- `check_ext_intact` ×2 (diagnóstico marcado "remove once the writer is
  found").
- 2 lecturas de `timer_now()` + `perf::record` con 2 `fetch_add` sobre
  cachelines **globales compartidas** (`SYS_COUNT[num]`/`SYS_NS[num]`
  ping-ponean entre cores en SMP).

### 5.6 ALTO — Lookups de ruta: sin caché, y el CWD se re-recorre siempre

`linux-object/src/fs/mod.rs:1620-1624`: con `AT_FDCWD` se recorre el CWD
desde la raíz **incluso para rutas absolutas** (cuyo resultado se tira);
`current_working_directory()` asigna un String por llamada; el walk asigna
2 Strings + `metadata()` + `find()` **por componente**
(`vendor/rcore-fs/src/vfs.rs:148-193`); no hay dentry cache. Un linker
dinámico hace 100-400 `openat/stat` por lanzamiento de proceso, y `sys_openat`
añade 2 `metadata()` más (`fd.rs:258,267`).

### 5.7 ALTO — `execve` lee el binario entero con doble copia y mapeo kernel

`fs/mod.rs:1468-1491` (`read_as_vmo`): lectura completa en trozos de 16 KiB
con doble copia (fs → buf → frames del VMO), luego el loader mapea el binario
entero en el espacio kernel y lo desmapea página a página con shootdown
(`loader/mod.rs:87-101`). El `ELF_VMO_CACHE` mitiga repeticiones pero con tope
de 8 MiB/archivo (un binario mayor se relee entero en cada exec) y evicción
`fifo.remove(0)` O(n).

### 5.8 MEDIO — Pipes y unix sockets mueven datos byte a byte

`pipe.rs:163-183`, `unix.rs:424-426`: bucles `pop_front()`/`push_back()` por
byte con el mutex del pipe en mano — un write de 64 KiB son 65 536
`push_back`s (~10-20× más lento que `memcpy`; `VecDeque` tiene
`as_slices`/`extend_from_slice`). El buffer del pipe además es ilimitado
(la capacidad es solo consultiva, `pipe.rs:39-43`).

### 5.9 MEDIO — Otros

- `recvfrom` asigna y pone a cero hasta 64 KiB por llamada (`net.rs:423-424`).
- `read`/`read_at` son `#[async_trait]` → un `Box::pin` por llamada, incluso
  para 1 byte.
- `accept` TCP reintenta cada 5 ms; `signalfd` cada 20 ms; `F_SETLKW` cada
  10 ms; `tty_write_out` gira con Ctrl-S activo (`stdio.rs:1490-1495`).
- `getdents64`: hasta 256 KiB de buffer por llamada sobre un walk **O(n²)**
  (ramfs usa `keys().nth(i)`; el default VFS añade `find(name)` por entrada).
- Listas de mapeos del VMAR: `Vec` lineal — cada `mmap/munmap/mprotect` es
  O(n) con cientos de entradas por proceso de escritorio (labwc: 596), la
  secuencia de arranque de un proceso es O(n²).
- `EventBus::set()` asigna un `Vec` por evento de readiness
  (`event_bus.rs:102-112`).

---

## 6. Cómo medir en tu máquina (ya integrado en el kernel)

- `cat /proc/perf` — contabilidad de syscalls (llamadas + tiempo).
- `cat /proc/perf/kernel` — idle vs busy por CPU, ticks, IRQs por vector,
  estadísticas de scheduler (`sched_stats`), preempción por wakeup, hit-rate
  del idle-callback.
- `cat /proc/perf/top` — perfilador de muestreo propio ("perf top").
- `HEAPPROF=1` en cmdline — ciclos por alloc/dealloc del heap.
- `WAKEPREEMPT=0` / `TIMERDEADLINE=0` en cmdline — comparar A/B las dos
  políticas de scheduler/timer en el mismo binario.
- `dmesg` — el ring klog de 8 MiB retiene el trazado de arranque completo.

Sugerencia de validación tras cada arreglo de §1/§2: `time cat archivo_1000_lineas`,
`dd if=/dev/vda of=/dev/null bs=1M count=64`, y un lookup DNS con la red caída.

---

## 7. Plan de acción priorizado (ratio ganancia/riesgo)

1. **[APLICADO]** Quitar `|| vmo.name() != ""` en `vmar.rs:490` (§5.1) —
   restaura el demand paging de todas las bibliotecas.
2. **[APLICADO]** PAT + mapeo WC del framebuffer (§1.1) — `pat.rs` programa
   la entrada PAT 7 como WC en BSP+APs y retipa las PTE del physmap del fb;
   `set_flags` emite el bit PAT para futuros mapeos WC.
3. **[APLICADO]** Consola: coalescing de presents a ~60 Hz con vaciado desde
   el tick (§1.2), guard de igualdad de celda (§1.4) y scroll de celdas con
   `rotate_left` + reciclado del historial (§1.6). (El present tras un
   scroll sigue siendo de pantalla completa — todos los píxeles cambian —
   pero ahora se paga ≤60 veces/s y sobre memoria WC.)
4. **[APLICADO]** poll/select/epoll aparcando wakers en los fds
   (`FileLike::subscribe_readiness`: pipes, unix sockets, ptys, eventfd,
   timerfd, DRM) con backstop estirado a 100 ms con cobertura completa
   (§5.2). De regalo: `Pipe::drop` ya no latchea CLOSED con extremos
   dup'eados vivos.
5. **[APLICADO parcial]** `tee_x_diag` limitado a fd ≤ 2 (§5.4) — los writes
   de pipes/sockets/archivos ya no pagan el escaneo; hunter/anomaly (§5.5)
   tiene ahora un knob A/B (`HUNTER_ANOMALY=0` en cmdline) para medir su
   coste en el mismo binario — el default sigue activado (postura de
   seguridad); el rediseño lock-free queda pendiente.
6. **DNS async** (§3.1): sustituir `spin_ms` por `NetRxOrTimeoutFuture` y
   saltarse la AAAA cuando la A ya respondió.
7. **[APLICADO]** Fault-around de 16 páginas en `VmMapping::handle_page_fault`
   (§5.3): en fallos de lectura/ejecución se pre-comitean y mapean read-only
   las 15 páginas siguientes (CoW preservado; páginas ya mapeadas nunca se
   degradan; revalidación anti-`cut()` idéntica a la página principal).
   Además (§5.6, aplicado): las rutas absolutas ya no re-recorren el CWD, y
   hay una caché ruta→inodo con invalidación por época en los syscalls
   mutadores (los pseudo-fs /proc//sys//dev quedan fuera por resolverse en
   `lookup_virtual_fs`). El arranque de sesión (ldso/xkb: cientos de
   open/stat absolutos repetidos) pasa a servirse de la caché.
8. **Subir el tamaño de bloque efectivo de la capa FS a ≥4 KiB** (§2.5) —
   divide por 8 el coste de AHCI/NVMe/virtio — y **virtio-blk multi-sector**
   (§2.3).
9. **AHCI**: habilitar interrupciones de puerto + soltar el lock durante la
   espera (§2.1); reactivar zero-copy y quitar el triple clflush (§2.2).
10. **`write()` TCP async** (§3.2) y recorte de `drain_net_poll(128)` → ≤4
    (§3.4); parqueo con timer en ARP/NDP + buffer RX reutilizable (§3.3).
11. **[APLICADO]** Executors bajo demanda / por CPU real (§4.1) — cada slot
    de `GLOBAL_RUNTIME` es ahora un `Once` que construye su runtime la
    primera vez que SU CPU entra al scheduler (el BSP en `warm_runtimes`,
    los AP en `run_until_idle`, en paralelo); las CPUs ausentes no
    construyen nada. Recupera ~157 MiB de heap con SMP=4 y quita ~134 MiB
    de stores de envenenado + ~8 600 desmapeos del arranque del BSP. Las
    sondas cross-CPU/IRQ usan un accessor no-forzante (slot sin construir
    = "nada que robar/despertar").
12. **[APLICADO]** Pipes/unix sockets con `as_slices`+`copy_from_slice` en
    la lectura y la especialización de slices Copy en la escritura (§5.8) —
    cada mensaje Wayland labwc↔clientes era un bucle byte a byte bajo un
    spinlock IRQ-off. La caché de CWD/rutas absolutas (§5.6) ya estaba
    aplicada en el punto 7.
13. **[APLICADO parcial]** Copia troceada del page-flip DRM (§1.3): el blit
    va ahora en bandas de 32 filas con ventana de IRQs entre bandas —
    latencia de interrupción acotada a ~una banda en vez de varios ms por
    frame, conservando el arreglo de anidamiento acotado que motivó el
    `intr_off`. DIRTYFB sigue sin implementar: wlroots/labwc presenta por
    page-flip (no llama a DIRTYFB), así que hoy no recortaría nada.
14. **Bit G en PTEs hoja del kernel (o PCID)** (§4.2).
15. Bajo demanda: UART TX por IRQ THRE, `fill_solid`/`fill_contiguous` en
    `ShadowDraw`, dirty por fila, `rotate_left` en el scroll de celdas, flush
    con rect en `DisplayScheme`, NVMe con QD>1 e IRQs, getdents O(n),
    mapeos VMAR en estructura ordenada.

---

## 8. Adenda 2026-08-11 — auditoría de ARRANQUE (encendido → escritorio labwc)

Cinco agentes auditaron el camino completo de arranque con la pregunta
"¿por qué Linux llega al escritorio en pocos segundos y Eclipse no?".
Resultado: no hay una bala única — son ~8 costes de 0,5–3 s apilados en tres
capas (rboot ~2,5–7 s, kernel ~3,5–4 s, userspace ~4–10 s).

### Aplicado en esta pasada

- **[APLICADO] Strip del ELF en la ESP**: rboot leía el zcore.elf completo
  (137 MB, ~12 MB cargables) por el driver FAT de UEFI a 50–200 MB/s.
  `objcopy --strip-debug` en la copia del Makefile: 137 MB → 13,3 MB
  (−0,6 a −2,5 s). El ELF completo sigue en `target/` para addr2line.
- **[REVERTIDO durante la caza del input — EXONERADO: la causa real era la
  limpieza de /run/udev, ver arriba. Re-aplicable con prueba de hardware]
  AHCI**: POD|SUD se encendía en todos los puertos implementados
  ANTES del wait global de presencia, y el wait baja de 2 s a 300 ms. Una
  controladora SATA vacía (desktop que arranca por NVMe) quemaba los 2 s
  completos en cada boot (−2 s). Los waits por puerto de `port.init()` no
  cambian.
- **[APLICADO] Logs PCI a debug**: "pci device enable done" + "failed to
  initialize PCI device" (NotSupported) + mapeo de BARs salían por UART
  115200 con spin por byte una vez POR FUNCION PCI (~0,5–1 s en placas con
  COM físico).
- **[REVERTIDO durante la caza del input — EXONERADO, misma causa real de
  arriba] xHCI**: la ventana de 100 ms de VBUS dejaba de ser un spin
  síncrono en el probe (×2 controladoras: chipset + el xHCI USB-C de la
  RTX); pasaba a ser un deadline que la enumeración diferida comprobaba en
  el primer poll. OJO al re-aplicar: `f8bed44` movió la enumeración HID de
  vuelta AL PROBE (síncrona), así que el diferido de VBUS habría que
  rediseñarlo sobre esa base, no re-aplicar el parche antiguo tal cual. El
  fix multi-instancia + latch por controladora (revertido en `2964a75`)
  sigue siendo arquitecturalmente correcto y re-aplicable por sí solo.
- **[APLICADO] hunter opt-in**: las heurísticas de tasa (mutex IRQ-off +
  BTreeMap + reloj en CADA syscall) pasan a off por defecto
  (`HUNTERANOMALY=1` para reactivar). El WATCH de syscalls sensibles sigue on.
- **[APLICADO] Stack de exec lazy** (`map_range=false`): 512 KiB de zero-fill
  + 128 PTEs menos por spawn (~60–100 spawns/boot), y forks posteriores dejan
  de re-caminar esas páginas.
- **[APLICADO] Read-ahead con 8 streams** (antes 4): el bring-up de sesión
  intercala más de 4 streams secuenciales y se desalojaban entre sí,
  degradando a lecturas de 4 KiB por comando.
- **[APLICADO] dcache 4096 entradas** (antes 1024, que se vaciaba ENTERO en
  plena tormenta de ldso+xkb+fontconfig).
- **[APLICADO] NVMe con lista PRP + bounce de 128 KiB** (antes 2 páginas =
  8 KiB por comando, QD1: 128 comandos serializados por ventana de 1 MiB,
  más lento que SATA). Clampado al MDTS anunciado por la controladora.
- **[APLICADO] eclipse-init `wait_socket`**: espera nativa (poll de stat a
  10 ms, cero forks) de seatd/wayland-0; los wrappers forkeaban un busybox
  `sleep 0.1` por iteración (~40–80 spawns en la ventana más caliente).
- **[APLICADO — CAUSÓ la regresión de input, ya corregida] /run y /tmp
  limpiados por init al arrancar**: los mounts de tmpfs son no-ops, y en
  btrfs instalado los sockets stale del boot anterior colgaban el arranque
  de sesión con backoffs. POSTMORTEM: la limpieza de `/run` borraba
  `/run/udev/data/c13:*`, la BD udev sintética que el kernel escribe para
  que libinput (sin udevd) considere inicializados los
  `/dev/input/event*`. Resultado: labwc arrancaba con CERO dispositivos
  (teclado+ratón+click muertos toda la sesión; la consola VT no pasa por
  udev y seguía viva). Los reverts de xhci/ahci nunca podían arreglarlo.
  Corregido en `f82835c` (clean_runtime_dir preserva `/run/udev`) +
  `f8bed44` (enumeración HID en el probe del xHCI) + `33a1962` (labwc
  espera a que /dev/input se asiente).
- **[APLICADO] Sonda serial solo en VT 0** (el loader exporta `ECLIPSE_VT`):
  los VT 1–5 pagaban 0,3 s de dd bloqueante + ~6 forks cada uno sin poder
  recibir respuesta jamás.
- **[APLICADO] udhcpc sin NIC**: sale en ~1 s para clasificarse como
  "crashing" y recibir backoff exponencial (antes reseteaba el backoff y
  re-corría ~5 forks cada 3 s para siempre).
- **[APLICADO] Fontconfig**: las ~400 fuentes bitmap de X11 (misc/cursor/
  encodings) fuera del escaneo vía `<rejectfont>` (el primer frame de labwc
  pagaba abrir+gunzip+parsear ~500 archivos); `/var/cache/fontconfig`
  pre-creado para que la caché de primer uso persista en btrfs.
- **[APLICADO] `SectorCache` de `BlockByteDevice` eliminada**: era 100 %
  sombra bajo `CachedDevice` (la capa de arriba absorbe toda repetición),
  así que cada MiB frío pagaba el doble de boxes/BTreeMap/copias por una
  caché que jamás podía acertar.

- **[REVERTIDO tras fallo en hardware] rboot con páginas de 2 MiB + BSS en
  bloque**: petaba el arranque al ~50% de la barra en la máquina real (causa
  exacta sin aislar: physmap 2MiB, BSS en bloque o la recogida de rangos).
  Re-introducir DE UNA PIEZA EN UNA con prueba en hardware entre cada una.
  Diseño original: el physmap se
  mapea con hojas de 2 MiB (512× menos `map_to`; sin invlpg por página —
  VAs nunca accedidas y CR3 se recarga en el handoff), con carve-out de
  4 KiB sobre el rango del framebuffer GOP para que el retipado WC de
  `pat.rs` siga funcionando, y fallback por-chunk a 4 KiB si el firmware ya
  tiene una entrada en conflicto. El BSS del kernel (~523 MiB) se asigna en
  los runs contiguos más grandes que el firmware conceda (a la mitad en cada
  fallo) en vez de UNA llamada `allocate_pages` POR FRAME (~134k llamadas),
  manteniendo PTEs de 4 KiB (stack_guard perfora agujeros de 4 KiB ahí).
  Verificado del lado kernel antes de tocar rboot: `PageTable::query`
  resuelve hojas de 2 MiB (los BARs PCI bajo el physmap siguen
  funcionando), `reserve_active_page_table_frames` salta entradas PS, y
  `pat.rs` ya rehusaba hojas huge (por eso el carve-out del fb).

### Pendiente (por ganancia/riesgo, de la misma auditoría)

1. **Líneas de 4 KiB en `BlockCache`** (la mitad restante del punto de la
   caché doble; la `SectorCache` redundante ya se borró): las líneas de
   512 B siguen costando ~2.050 boxes + ~4.000 ops de BTreeMap por MiB
   frío. Pasarlas a 4 KiB (insertar solo líneas completas; `patch` solo
   sobre residentes preserva el invariante caché==disco) las divide por 8.
2. **e1000e**: `reset_and_init` (ULP/LANPHYPC, ~0,5–0,7 s en i219 real)
   corre síncrono en el probe. Diferirlo exige que el probe devuelva el
   Device con el hardware sin resetear y registre el reset como deferred job
   (cuidado con el orden de MSI) — cirugía real, solo paga en i219.
3. **64 executors eager** (§4.1, sigue vivo): ~168 MiB de heap muertos con
   4–16 CPUs reales + ~60–115 ms de boot. Construcción por-CPU en el primer
   `run_until_idle`.
4. **exec con caché de VMOs de segmentos** (por inode): cada exec copia la
   imagen 3 veces (~1–3 ms × 60–100 spawns) aunque el archivo esté cacheado.
5. **Teardown de aspace con gather**: hoy un IPI a todas las CPUs POR
   MAPPING en exit/exec (~30–120 ms/boot); `fork_from` ya tiene el patrón.
6. **6 GraphicConsoles eager** (50 MB a 1080p, 200 MB a 4K de heap): VT 1–5
   lazy en el primer switch.
7. **DNS async** (§3.1, latente): 6,4–9,6 s de spin a 100 % de CPU por
   lookup fallido — dormido en el boot por defecto, despierta con el primer
   cliente que resuelva nombres.
8. Menores: pipes byte a byte (§5.8), AP bring-up 10 ms/AP, PIT 55 ms,
   clflush del CQE de NVMe por iteración de poll, GSP del segundo GPU
   síncrono (multi-GPU), `set_poll_instance` de xHCI mono-slot (la
   controladora vacía de la GPU puede pisar a la del chipset — bug funcional
   a revisar, no de rendimiento).

## §9. Ingeniería inversa del arranque de labwc (medir → precargar)

labwc llega a su primer frame solo tras: (a) que ld.so mapee ~30-40 objetos
compartidos (libwlroots arrastra libinput, libdrm, wayland, xkbcommon,
pixman, pango, cairo, glib...), (b) que xkbcommon parsee el keymap de
docenas de ficheros minúsculos de `/usr/share/X11/xkb`, y (c) fontconfig /
tema / config. En frío, todo eso es una tormenta de fallos de página
dispersos de 4 KiB sobre ficheros que nunca estuvieron en la caché de
bloques. La maquinaria de paginación ya es correcta (mmap demand-paged,
caché de página por inode, fault-around de 16 páginas), así que el ataque no
es más paginación: es **precargar el working set antes de que labwc arranque**,
usando la ventana muerta que ya gastamos esperando a seatd y a que
`/dev/input` se asiente. Es la técnica de `ureadahead`/`preload` de Linux.

Para precargar hay que saber QUÉ precargar. De ahí la ingeniería inversa.

### §9.1. Grabador de arranque — `BOOTTRACE=<comm>` → `/proc/bootprofile` [APLICADO]

`linux-object/src/boot_trace.rs`: gateado por completo tras un flag de
cmdline. `BOOTTRACE=labwc` arma el grabador para el primer proceso cuyo
`comm` coincida; a partir de ahí registra cada `open`/`openat` de ESE pid
(path + marca de tiempo + resultado, incluidos los ENOENT que revelan cómo
ld.so sondea su ruta de búsqueda de librerías) en un anillo acotado en RAM
(8192 entradas). Se engancha en `sys_openat` envolviendo el cuerpo en un
closure para que sus muchos `?`/`return` desemboquen en un solo `ret` que el
grabador ve. Coste cuando está apagado (por defecto): **una carga atómica
relaxed por openat**, cero asignaciones.

`/proc/bootprofile` renderiza: cabecera (pid, uptime de arranque, ventana,
nº de opens ok/miss), un **timeline** con el delta entre opens y un `*` en
los huecos > 2 ms (el trabajo de CPU / esperas bloqueantes — p.ej. la
compilación del keymap xkb, o el primer render, se ven como huecos), y por
último la **preload list**: los paths abiertos con éxito, deduplicados, en
orden de primer acceso. Esa lista es materia prima directa de la fase de
prefetch (`boot_trace::preload_list()` la devuelve programáticamente).

Uso: `make ... BOOTTRACE=labwc`, arrancar hasta el escritorio, luego
`cat /proc/bootprofile`. El Makefile lo añade a la cmdline vía el flag
`BOOTTRACE=` (espejo de `DESKTOP=`), así que es opt-in sin tocar la ESP.

### §9.2. Prefetch del working set [PENDIENTE — segunda mitad]

Con la preload list capturada en hardware real, eclipse-init la lee en la
caché de bloques en una pasada secuencial durante la ventana de seatd +
asentamiento de `/dev/input` (que hoy es tiempo muerto), en paralelo con la
enumeración de input. Cuando labwc arranca y falla las páginas, todo está
caliente. Diseño ureadahead: 1er arranque graba, 2º en adelante precarga.
Alternativa a medir: sub-lista por fases (primero librerías, luego xkb).

### §9.3. Caché de keymap xkb precompilado [PENDIENTE — candidato]

Si el timeline confirma que la compilación del keymap xkb es una rebanada
grande (hueco largo tras abrir los ficheros de `/usr/share/X11/xkb`),
compilar el keymap en build-time y apuntar wlroots a él por env, saltándose
el parseo en cada arranque.

### §9.4. RESULTADO de la primera medición en hardware real [DATO]

Traza de `/proc/bootprofile` con `BOOTTRACE=labwc` en la máquina del usuario
(dual RTX 2060). **labwc tarda ~229 s** de la primera apertura a `menu.xml`:

| Fase | Rango | Dur | Qué |
|------|-------|-----|-----|
| Carga de librerías (ld.so) | 0–3,7s | ~3,7s | ~90 `.so`; cada una probada en `/lib` (ENOENT) y luego `/usr/lib` (ok) |
| Hueco a fontconfig | 3,7–7,7s | ~4s | stall |
| **fontconfig escaneo en frío** | **7,7–~117s** | **~109s** | **cuello de botella #1** |
| xkb + cursores + .desktop | 117–134s | ~17s | |
| **stall → icon-theme.cache** | 134–168,8s | **~35s** | un hueco sin I/O |
| libinput quirks + input | 168,8–175s | ~6s | |
| tema + shm + fuentes | 175–187s | ~12s | |
| **stall → menu.xml** | 187–228,9s | **~42s** | un hueco sin I/O |

Dos hallazgos:

1. **fontconfig escanea ~800 fuentes bitmap de X11 que nadie usa** (`misc/
   *.pcf.gz` en todas las codificaciones, `75dpi`, `100dpi`, `encodings`,
   `cyrillic`), parseándolas en frío para construir la caché — ~109 s. El
   `rejectfont` de la tarea #75 NO lo evitaba: rechaza al emparejar, no al
   escanear. **CORREGIDO**: `fonts.conf` propio que solo lista los dirs
   escalables que usa el escritorio (dejavu, Adwaita); las bitmap quedan en
   disco para el FontPath de Xorg pero fuera del escaneo de fontconfig
   (`xtask/src/linux/xorg.rs`). Debería eliminar el grueso de los 109 s y,
   al no reventar la dcache con 800 ficheros, acelerar TODO lo que va detrás.

2. **Cada apertura de fichero cuesta 100–600 ms, incluso los ENOENT** (los
   `._foo` que fallan tardan ~300 ms sin datos que leer). Multiplicador
   sistémico: dcache reventando bajo la tormenta de fontconfig + metadatos
   fríos. Probablemente se alivie mucho al quitar la tormenta (hallazgo 1);
   lo que quede se ataca con la caché de bloques de 4 KiB (pendiente #1) y/o
   prefetch (§9.2).

Los dos stalls de 35 s y 42 s NO tienen actividad de fichero (CPU o espera
bloqueante). Para verlos, el grabador se **extendió** para registrar también
las syscalls lentas (>300 ms) del proceso, que aparecen como líneas `SLOW
<syscall> took <ms>` en el timeline. La próxima traza dirá si esos huecos son
render (pixman), espera de GPU (ioctl), o poll bloqueante.

## §10. Renderer GL por defecto (y cómo volver a pixman)

Desde el commit del interruptor de renderer, el **defecto es GL** (`GL ?= 1`
en `zCore/Makefile`). El build estampa `renderer=gl` en la cmdline y `make
qemu` arranca QEMU con `-device virtio-vga-gl -display gtk,gl=on`. eclipse-init
(`build_child_env`) NO fija `WLR_RENDERER=pixman` cuando ve `renderer=gl`, así
que wlroots/Mesa auto-seleccionan la GPU: `virtio_gpu_dri.so` en QEMU (virgl),
`nouveau_dri.so` en la RTX real. Los drivers DRI ya viajan en la imagen
(antes se excluían por error, rompiendo "virtio_gpu: driver missing").

**Cómo quitar GL / volver a pixman** (el renderer software por CPU, que
renderiza en QEMU y en hardware real hoy):

```
make ... GL=0
```

`GL=0` estampa `renderer=pixman` en la cmdline y deja QEMU en `-vga virtio`.
Úsalo cuando:
- el host no tiene `virglrenderer` (QEMU no arrancaría `virtio-vga-gl`), o
- en hardware real, mientras la pila GL de nouveau todavía no componga un
  frame (GL dejaría el escritorio en negro; pixman siempre pinta).

**Hardware real**: la cmdline de la ESP (`rboot.conf`) se escribe desde
`CMDLINE`, así que un build por defecto (GL) también estampa `renderer=gl`
ahí. Construye la imagen instalada con `GL=0` hasta que nouveau GL renderice,
o edita a mano `renderer=gl` → `renderer=pixman` en `rboot.conf`.

**A mano en un arranque concreto**: añade/quita el token `renderer=gl` o
`renderer=pixman` en la línea `cmdline=` de la ESP; eclipse-init lo lee de
`/proc/cmdline` en cada arranque, sin reconstruir nada.

## §11. AVISO CRÍTICO: las medidas de QEMU eran bajo EMULACIÓN (TCG, sin KVM)

`make qemu` corre por defecto con `-cpu Haswell` bajo **TCG** (emulación pura
de CPU): `ACCEL ?=` está vacío, y `-accel kvm -cpu host` solo se añade con
`ACCEL=1` (o `HYPERVISOR=1`). Ver `zCore/Makefile` líneas 27/194/224/302-306.

Consecuencia: **todas las medidas de arranque en QEMU están infladas ~10-50×**
por la emulación de CPU. El camino GL lo sufre muchísimo más que pixman porque
compilar shaders (Mesa) es CPU-intensivo — exactamente lo que TCG emula peor.
Traza GL en TCG: `readlink` 3,7s, `mmap` anónimo 300-900ms, esperas `futex`
de hasta 49s (el hilo principal de Mesa esperando a los de compilación),
ventana total ~868s. Bajo KVM esas operaciones son de milisegundos.

**Para medir/usar de verdad en QEMU**: `make qemu ... ACCEL=1` (necesita
`/dev/kvm`; en POP-OS, el usuario en el grupo `kvm`). Da `-accel kvm -cpu
host` = CPU casi nativa. En hardware real no aplica (ya es nativo).

### Cuellos de kernel reales que quedan (independientes de TCG, re-medir con KVM)

1. **`VmarInner.mappings` es un `Vec`** (`zircon-object/src/vm/vmar.rs:237`):
   `map_ext_min` (`determine_offset` + `test_map`) y `find_mapping` hacen scan
   lineal O(n). Con los miles de mappings anónimos de Mesa es O(n²). Pasarlo a
   `BTreeMap` por dirección lo baja a O(log n). Real incluso en nativo, pero el
   constante es µs — bajo TCG se magnifica. Cirugía de VM core: alto riesgo,
   requiere prueba cuidadosa (find_mapping/unmap/fork/teardown).
2. **`readlink`/`stat`/`open` de sysfs lentos** (libdrm re-sondea la PCI de la
   GPU repetidamente): re-medir con KVM antes de decidir si es coste real de la
   implementación de sysfs o solo inflado de TCG.
