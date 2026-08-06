# e1000e: auditoría de bugs (2026-08-06)

Barrido de `drivers/src/net/e1000e.rs` (2796 líneas) buscando fallos
funcionales concretos: pérdida/corrupción de paquetes, UB, deadlocks,
programación incorrecta del hardware o estado inconsistente. No es una
pasada de estilo — cada punto tiene un escenario de fallo reproducible por
lectura de código, o una comparación directa con el driver hermano
`e1000.rs` y con el mapa de registros real de Intel/Linux e1000e.

**Estado: los 13 hallazgos están corregidos** en `drivers/src/net/e1000e.rs`.
Verificado con `cargo test -p zcore-drivers --lib --features mock e1000e`
(11 tests, incluyendo un nuevo módulo `tx_ring_tests` que cubre el cambio de
detección de fin de TX) y `cargo build -p zcore-drivers` (build `no_std`
real). Este documento se mantiene como registro histórico de la auditoría;
cada sección de abajo describe el bug tal como se encontró.

## Crítico

### 1. `static mut ROUTES_STORAGE` — UB por aliasing + corrupción de rutas entre NICs
`e1000e.rs:2189-2191`

```rust
static mut ROUTES_STORAGE: [Option<(IpCidr, Route)>; 4] = [None; 4];
let mut routes = unsafe { Routes::new(&mut ROUTES_STORAGE[..]) };
```

`init()` se ejecuta una vez por cada NIC detectada por PCI. Cada llamada
toma un `&'static mut` sobre el **mismo** almacenamiento: con dos e1000e en
la máquina hay dos referencias exclusivas vivas simultáneamente a la misma
memoria (UB en el modelo de aliasing de Rust) y, en la práctica, la tabla
de rutas de una tarjeta pisa la de la otra (`add_route`/`del_route` de una
interfaz corrompe el estado de la otra).

## Alto

### 2. Detección de fin de TX vía lectura de TDH en vez del bit DD
`e1000e.rs:1398-1410`, `1456`

```rust
fn tx_slots_free(&self) -> usize {
    let head = unsafe { mmio_read(self.base, E1000E_TDH) as usize };
    ...
}
```

`tx_slots_free()` calcula slots libres leyendo TDH por MMIO en vez de
comprobar el bit DD del descriptor (que sí se activa, porque se marca
`TX_CMD_RS`). El datasheet de Intel advierte que TDH refleja el prefetch
interno del NIC, no la finalización real de la transmisión. El driver
hermano `e1000.rs` sí usa el bit DD (`status & 1`, líneas 276 y 311) — aquí
nunca se lee. Invisible bajo QEMU (completa TX de forma síncrona); en
hardware real puede reutilizar un buffer/descriptor antes de que el DMA
haya terminado de leerlo → corrupción o pérdida de frames en ráfagas TX
sostenidas.

### 3. Offsets de FEXTNVM6/FEXTNVM7 incorrectos
`e1000e.rs:64-65`

```rust
const E1000E_FEXTNVM6: usize = 0x01014 / 4;
const E1000E_FEXTNVM7: usize = 0x01018 / 4;
```

Según el mapa de registros de Linux e1000e, FEXTNVM6 está en `0x00010` y
FEXTNVM7 en `0x000E4` — no cerca de PBA (`0x1000`). El archivo muerto
`e1000e_pch.rs:30` tiene el mismo `0x01018` erróneo, lo que sugiere un
origen compartido del error (posible confusión con la región de PBA).
Efecto: los workarounds de ULP/SPT (líneas 683-684, 995-996) hacen
read-modify-write sobre MMIO reservado en vez de tocar el registro real, y
`FEXTNVM7.DISABLE_SMB_PERST` nunca se limpia de verdad — en hardware real
sin firmware ME activo, el PHY puede quedar atascado en modo SMBus tras un
ciclo de PERST#/suspend.

*Nota de confianza:* verificado por comparación cruzada de código (mismo
error en dos archivos) y por conocimiento del mapa de registros de Linux
e1000e; conviene contrastarlo contra el datasheet oficial de Intel antes de
tocarlo.

### 4. `RCTL_SECRC` solo se activa si `is_pch()`
`e1000e.rs:1215-1218`

```rust
let mut rctl = RCTL_EN | RCTL_UPE | RCTL_MPE | RCTL_BAM;
if self.is_pch() { rctl |= RCTL_SECRC; }
```

SECRC (strip FCS) es un bit RCTL estándar en toda la familia 8254x/e1000e,
y `e1000.rs:228` lo activa incondicionalmente. En 82574L y en el propio
modelo `e1000e` de QEMU (no-PCH), cada frame entregado a smoltcp/AF_PACKET
lleva los 4 bytes de FCS pegados al final del payload.

### 5. `matched()` incluye IDs de la familia igb (I210/I211) que el resto del driver no soporta
`e1000e.rs:2240-2241` vs `is_pch()` / `is_pch_spt_or_later()`

`0x1533` (I210), `0x1539` (I211), `0x157b`/`0x157c` (I210 flashless) hacen
match en `matched()`, pero `is_pch_spt_or_later()` los excluye, así que
`RXDCTL.QUEUE_ENABLE` nunca se programa para ellos. En la familia igb, sin
ese bit la cola no arranca aunque `RCTL.EN` esté puesto: la tarjeta queda
completamente muerta (RX y TX) sin ningún error reportado — el driver
reclama soporte que no puede entregar.

### 6. El trabajo diferido puede evictarse dejando `poll_pending`/IMS enmascarado para siempre
`e1000e.rs:1735` + `drivers/src/utils/deferred_job.rs:36-40`

La cola global de jobs diferidos tiene cupo 256 y `evict_oldest_job`
descarta el más antiguo sin ejecutarlo. Solo el propio job de e1000e pone
`poll_pending=false` y rearma IMS (líneas 1745-1746); si lo evictan bajo
presión de otros drivers, no hay ningún camino de recuperación — la NIC
deja de recibir interrupciones permanentemente. El mismo patrón afecta a
`watchdog_job_scheduled` (línea 1606/1617-1618): un watchdog evictado mata
la vigilancia de enlace para siempre.

## Medio

### 7. Reensamblado multi-descriptor inalcanzable y sin contabilizar
`e1000e.rs:1316-1327`

Un fragmento no-EOP siempre llena el buffer completo (2048 B), así que
`pending.len() + len > BUF_SIZE` es **siempre verdadero** en el segundo
fragmento — la rama que fusiona (línea 1321) nunca se ejecuta, y el frame
se descarta **sin** incrementar `rx_dropped` (a diferencia de los otros
descartes, líneas 1287/1297). Peor: para un frame de 3+ descriptores, el
último fragmento se entrega solo, como si fuera el paquete completo
(contado como `rx_packets` válido). Hoy está inerte porque `RCTL.LPE` no se
activa (frames ≤1522B caben en un solo buffer de 2048B), pero es una bomba
de tiempo latente para cualquier futuro soporte de jumbo frames.

### 8. `TX_DROPPED`/`RX_CSUM_BAD` son estáticos globales del archivo
`e1000e.rs:345, 351`

Compartidos por todas las instancias de e1000e. Con 2 NICs, el contador de
"ACK perdido" del watchdog de una tarjeta (añadido específicamente para
diagnosticar el deadlock de descarga silenciosa) refleja los descartes de
la otra tarjeta.

### 9. `name = format!("eth{}", dev.loc.bus)` ignora device/function
`e1000e.rs:2274`

Dos funciones NIC en el mismo bus PCI (p. ej. una tarjeta multi-puerto)
reciben el mismo nombre de interfaz `ethN`.

### 10. `CTRL_FRCSPD`/`CTRL_FRCDPX` con los bits intercambiados
`e1000e.rs:119-120`

El valor real (datasheet/Linux) es `FRCSPD=bit11` (`0x800`),
`FRCDPX=bit12` (`0x1000`); aquí están al revés. Inofensivo hoy porque
ambos siempre se usan juntos (líneas 748, 1019), pero es una trampa para
cualquier código futuro que toque uno solo (p. ej. un workaround de
velocidad forzada).

## Bajo

- **`FWSM_FW_VALID` definida como bit 14 en vez de bit 15** (`e1000e.rs:202`) —
  la constante correcta (`ICH_FWSM_FW_VALID`, bit 15) coexiste sin usarse
  la primera; constante muerta y engañosa.
- **`TIPG` con IPGR2=12 en vez del valor de datasheet 6** (`e1000e.rs:1101`) —
  solo relevante en enlaces half-duplex, prácticamente extintos en Gigabit.
- **Timeout de latch de `TXDCTL.QUEUE_ENABLE` en SPT ignorado en silencio**
  (`e1000e.rs:1108-1113`) — si nunca se activa, el TX queda muerto sin
  ningún log de error ni fallo reportado por `init_tx`.

## Metodología y cobertura

Búsqueda por 6 lentes independientes (RX, TX, inicialización de hardware,
concurrencia, seguridad de memoria, integración con el resto del SO) sobre
`drivers/src/net/e1000e.rs`, comparando contra `e1000.rs` (driver hermano),
`drivers/src/utils/dma_sync.rs`, `drivers/src/utils/deferred_job.rs`, el
crate `lock` (`vendor/kernel-sync`) y el mapa de registros conocido de
Intel 82574/e1000e/I219 (PCH).

Las lentes de "seguridad de memoria" e "integración" no llegaron a
completar su verificación adversarial automática (falta de créditos del
modelo a mitad de sesión); los 10 hallazgos anteriores fueron verificados
manualmente releyendo el código y contrastándolo contra el driver hermano
y el conocimiento del mapa de registros de Intel/Linux — no contra el
datasheet oficial en vivo, así que las afirmaciones sobre offsets de
registro (hallazgos #3, #10, y los de baja severidad) conviene
contrastarlas antes de aplicar una corrección.

## Correcciones aplicadas

| # | Hallazgo | Corrección |
|---|----------|------------|
| 1 | `ROUTES_STORAGE` aliasing | `Box::leak` de un buffer fresco por NIC en vez de un `static mut` compartido. |
| 2 | TX completion vía TDH | `can_send()` lee el bit DD del descriptor en `tx_tail` (con `dma_sync` FromDevice) en vez de la aritmética TDH; `init_tx` pre-marca todos los descriptores TX con DD=1. |
| 3 | Offsets FEXTNVM6/7 | Corregidos a `0x00010`/`0x000E4` (mapa de registros real). |
| 4 | `RCTL_SECRC` solo en PCH | Ahora incondicional, como `e1000.rs`. |
| 5 | IDs igb en `matched()` | Se retiraron `0x1533`/`0x1539`/`0x157b`/`0x157c`. |
| 6 | Eviction de la cola diferida deja `poll_pending`/IMS atascados | `heal_stuck_poll_pending()`, llamado desde `NetScheme::poll()` (alcanzado por el polling periódico independiente de IRQ), limpia el flag si lleva >500 ms atascado. |
| 7 | Tope de reensamblado RX inalcanzable | Nuevo `MAX_RX_FRAME_BYTES = BUF_SIZE * 16`; el descarte por tope ahora también incrementa `rx_dropped`. |
| 8 | `TX_DROPPED`/`RX_CSUM_BAD` globales de archivo | Movidos a campos `tx_dropped`/`rx_csum_bad` de `E1000eHw` (por instancia). |
| 9 | Colisión de nombre `eth{bus}` | Se añade `_{device}_{function}` cuando no son ambos cero. |
| 10 | Bits `CTRL_FRCSPD`/`CTRL_FRCDPX` intercambiados | Corregidos a bit 11 / bit 12 respectivamente. |
| 11 | `FWSM_FW_VALID` con bit incorrecto | Constante duplicada y sin uso, eliminada (queda `ICH_FWSM_FW_VALID`, correcta). |
| 12 | `TIPG` IPGR2=12 | Corregido a 6 (valor de datasheet). |
| 13 | Timeout de `TXDCTL.QUEUE_ENABLE` silencioso | Ahora emite `klog_warn!` si no llega a activarse en 10 ms. |

Cobertura de test nueva: módulo `tx_ring_tests` (4 tests) que ejercita el
nuevo camino DD-bit de TX contra un NIC simulado — arranque con todos los
slots libres, reutilización bloqueada hasta el write-back de DD, uso del
anillo completo sin slot de guarda, y vuelta de anillo intercalada con
completions.

## Rendimiento (2026-08-06, segunda pasada)

Tras la corrección de bugs, el usuario reportó que el driver "va muy lento".
No era una regresión de los fixes anteriores — era un patrón preexistente en
el camino RX que nunca se había medido: **cero agrupamiento de accesos MMIO
por paquete**. Cada paquete recibido, sin importar cuántos llegaran en la
misma ráfaga, pagaba su propio round-trip MMIO completo.

### Lo que hacía antes, por paquete

En `process_rx_slot` / `receive` / `recycle_rx_slot`:

1. `process_rx_slot` releía RDH (`mmio_read`) al empezar, para comprobar si
   había algo nuevo que procesar.
2. Si no había paquete completo, `receive`'s bucle de drenaje **volvía a leer
   RDH** una segunda vez para decidir si cortar — hasta 2 lecturas MMIO de
   RDH por iteración.
3. Al reciclar el descriptor consumido, `recycle_rx_slot` hacía
   `mmio_write(RDT, i)` seguido de un `mmio_read(RDT)` de "flush" —
   **una lectura síncrona que fuerza al CPU a esperar la vuelta completa
   PCIe** — en cada paquete individual, nunca agrupado.
4. `ensure_rx_armed_if_link_up`, invocada en cada poll (con o sin tráfico),
   releía STATUS por MMIO incluso cuando el enlace ya se sabía activo.
5. La invalidación de caché del buffer RX (`dma_sync_region`, ruta
   write-back) siempre invalidaba los `BUF_SIZE` (2048) bytes completos del
   slot, sin importar que el frame real midiera 60 bytes (p. ej. un ACK) —
   hasta 32 líneas de caché invalidadas para leer 1.

En hardware real un `mmio_read` fuerza al CPU a esperar una transacción de
finalización PCIe (cientos de ns a varios µs según la topología). Bajo
emulación (QEMU, el objetivo de pruebas habitual de este driver) **cada
acceso MMIO — lectura o escritura — típicamente dispara una VM exit
completa**, con un coste de varios µs solo de entrada/salida del hipervisor,
antes de que el modelo del dispositivo haga nada. Con 3-4 transacciones MMIO
por paquete recibido, la sobrecarga de "contabilidad" dominaba por completo
el coste real de mover los bytes, sobre todo bajo ráfagas.

### Cambios aplicados

Todos en `drivers/src/net/e1000e.rs`, todos solo en el camino RX (el TX ya
posteaba el doorbell TDT sin lectura de flush, así que no tenía el mismo
patrón):

1. **`receive()` cachea RDH una sola vez por llamada** (`let rdh =
   self.rx_rdh();`) en vez de releerlo por MMIO en cada iteración del bucle
   de drenaje. `process_rx_slot` ya no relee RDH — el invariante (`head !=
   rdh`) lo garantiza quien llama. Un paquete que llegue justo después del
   snapshot simplemente se recoge en la siguiente llamada a `receive()`;
   es el comportamiento normal de un drenaje por presupuesto, no un fallo de
   corrección.
2. **El doorbell RDT se difiere y se agrupa** (`rx_doorbell_dirty` +
   `flush_rx_doorbell()`): reciclar un descriptor ya no escribe RDT
   inmediatamente, solo marca el flag. El flush real ocurre una vez por
   ráfaga — al final de `poll_with_irq_hint` (tras el `iface.poll()` de
   smoltcp, que puede haber drenado muchos paquetes) y al final de
   `NetScheme::recv()` (que no pasa por `poll_with_irq_hint`). Diferirlo es
   seguro: solo retrasa avisar al hardware de buffers ya liberados, nunca
   bloquea el avance de nada. Se eliminó también la lectura de flush
   síncrona — TX nunca la tuvo, tampoco hacía falta aquí.
3. **`ensure_rx_armed_if_link_up` no relee STATUS si `link_up` ya es true**
   — el único efecto de la función es ponerlo a `true`, así que si ya lo
   está, la lectura no hace nada. Las transiciones a enlace caído las sigue
   detectando `watchdog_tick` en su propio ciclo.
4. **La invalidación de caché del buffer RX usa `len` real, no `BUF_SIZE`**
   — nada lee más allá de `len` (el slice construido justo después es
   `..len`), así que invalidar el buffer completo en cada paquete no
   aportaba nada, solo coste.

### Verificación

Nuevo test `rx_doorbell_is_batched_not_rung_per_packet` que entrega 5
paquetes, drena los 5 sin flush intermedio y comprueba que RDT no se mueve
hasta llamar a `flush_rx_doorbell()` explícitamente, y que un segundo flush
sin nada nuevo reciclado es un no-op. `rx_single_packet_roundtrips` se
actualizó para reflejar el nuevo contrato (hay que flushear antes de mirar
RDT). Los 13 tests existentes (RX, TX, coherency bench) siguen en verde.
`cargo build -p zcore-drivers` (build `no_std` real) y `cargo clippy`
limpios.
