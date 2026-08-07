# Rendimiento de Eclipse frente a Linux: medición real y correcciones

Este documento explica el desfase entre lo que decía `eclipse-bench` y lo que se
percibía usando el sistema, **qué se midió realmente** arrancando Eclipse y
Linux bajo el mismo QEMU, y qué se ha cambiado en el núcleo.

## 1. El benchmark antiguo no medía el sistema operativo

La versión anterior de `tools/eclipse-bench` publicaba cuatro bloques: CPU,
memoria, disco y creación de procesos. Al leerlos, Eclipse parecía estar a la
altura de Linux. El benchmark no mentía sobre lo que medía — simplemente casi
nada de lo que medía dependía del núcleo.

| Bloque | Qué ejecutaba en realidad |
| --- | --- |
| `int/float latency`, `int throughput` | Bucles ALU en espacio de usuario. Cero código de núcleo. |
| `memcpy`, `memset`, `random access latency` | `memcpy` sobre búferes ya paginados. Cero código de núcleo. |
| `syscall getpid()` | La llamada al sistema más barata que existe. |
| `DISK` | En QEMU, la raíz SFS en RAM: la caché de páginas, no el disco. |
| `PROCESS` | Lo único genuinamente del núcleo — y aun así, un proceso cada vez. |

Y un segundo sesgo, más importante: **cada medición corría sola**. Esa es
precisamente la única condición bajo la cual un planificador no puede quedar en
evidencia, porque nunca hay una segunda tarea ejecutable esperando turno.

Faltaba por medir lo que domina cualquier carga real: latencia de despertar,
coste de cambio de contexto, fallos de página, `mmap`, copy-on-write tras
`fork`, resolución de rutas, `clock_gettime` y escalado SMP. Todo eso está ahora
en `eclipse-bench`, con cada fila etiquetada `[user]` (propiedad de la CPU) o
`[kernel]` (propiedad del sistema operativo).

## 2. Cómo medir de verdad

Dos arneses, misma máquina QEMU (mismo `-cpu`, `-smp`, `-m`, misma emulación
TCG sin KVM), mismo binario de userland:

```sh
# Preparación (una vez)
cargo rootfs --arch x86_64
make -C zCore build MODE=release LINUX=1 GRAPHIC= LOG=warn

# Eclipse
./scripts/qemu-bench.sh -o /tmp/ecl.log -t 3400 "cd /root && /bin/eclipse-bench --quick . 8 8"

# Linux, exactamente la misma binaria y la misma máquina emulada
./scripts/qemu-linux-bench.sh -o /tmp/lin.log -t 3400 "cd /root && /bin/eclipse-bench --quick . 8 8"
```

Comparar Eclipse emulado contra el host nativo no dice nada: compara una CPU
emulada con una real. Arrancar **los dos núcleos bajo la misma emulación**
cancela el hardware.

> Los arneses no usan KVM (los contenedores de compilación no suelen tener
> `/dev/kvm`). Los números absolutos son varias veces peores que en hardware
> real, pero son comparables entre sí. Las filas de **peor caso** tienen mucho
> ruido bajo TCG en una máquina compartida (se han observado variaciones de 2-3x
> entre corridas idénticas); las medias son estables.

## 3. Lo que se midió (Linux 6.8 vs Eclipse, mismo QEMU, 4 vCPU)

**El resultado invierte la premisa.** Eclipse no es lento en las llamadas al
sistema: es **2-5x más rápido que Linux** en casi todas ellas. Cifras tras
aplicar las correcciones de la sección 4.

| `[kernel]` | Eclipse | Linux 6.8 | |
| --- | ---: | ---: | --- |
| `getpid()` | 8 815 ns | 26 650 ns | Eclipse **3,0x** |
| `sigprocmask()` | 9 496 ns | 21 617 ns | Eclipse **2,3x** |
| `sched_yield()` | 13 186 ns | 33 290 ns | Eclipse **2,5x** |
| `pread(1 B)` | 12 379 ns | 31 440 ns | Eclipse **2,5x** |
| `write(1 B)` | 10 969 ns | 31 632 ns | Eclipse **2,9x** |
| `fstat()` | 10 521 ns | 39 175 ns | Eclipse **3,7x** |
| `stat("/dev/null")` | 16 469 ns | 70 114 ns | Eclipse **4,3x** |
| `open+close` | 49 869 ns | 140 907 ns | Eclipse **2,8x** |
| `mmap+munmap` | 55 867 ns | 177 671 ns | Eclipse **3,2x** |
| fallo de página menor | 15 915 ns | 89 968 ns | Eclipse **5,7x** |
| pipe ida y vuelta (procesos) | 220 us | 744 us | Eclipse **3,4x** |
| pipe ida y vuelta (hilos) | 79 us | 669 us | Eclipse **8,5x** |
| `nanosleep(1 ms)` retraso, ocioso, peor caso | 1 761 us | 1 836 us | Eclipse |
| `nanosleep(1 ms)` retraso, carga, peor caso | 3 245 us | 3 961 us | Eclipse |
| `nanosleep(1 ms)` retraso, carga, media | 975 us | 828 us | empate |
| latencia de despertar carga/ocioso (media) | **0,98x** | 1,63x | Eclipse |
| latencia de despertar carga/ocioso (peor) | **1,84x** | 2,16x | Eclipse |
| `fork + exit` | 3 708 us | 3 690 us | empate — *medido antes del COW; con COW activado son ~11 ms, ver 3.quater* |

Y donde Eclipse **sí** pierde — que es exactamente lo que se nota al usarlo:

| `[kernel]` | Eclipse | Linux 6.8 | |
| --- | ---: | ---: | --- |
| `clock_gettime(MONOTONIC)` | 8 597 ns | **199 ns** | Linux **43x** — *corregido, ver 3.quinquies* |
| `fork + exec(/bin/sh -c :)` | 46 447 us | 10 804 us | Linux **4,3x** |
| `fork + exec` (estático, 60 KiB) | 15 609 us | 9 270 us | Linux **1,7x** |
| pipe ida y vuelta bajo carga | 2 648 us | 1 363 us | Linux **1,9x** |
| `nanosleep(1 ms)` retraso, ocioso, media | 991 us | 507 us | Linux **2,0x** |
| `mprotect` | 183 360 ns | 101 111 ns | Linux **1,8x** |
| eficiencia SMP | 87,6 % | 98,0 % | Linux |

Esa es la respuesta a la pregunta original. El sistema no se siente lento porque
las llamadas al sistema lo sean; se siente lento por un puñado de rutas muy
concretas: leer la hora, lanzar un comando y la latencia bajo carga.

### Efecto de las correcciones

La latencia de temporizador era la peor brecha después de `clock_gettime`, y ha
pasado de 4,6x por detrás de Linux a paridad o mejor:

| | antes | después | Linux |
| --- | ---: | ---: | ---: |
| `nanosleep(1 ms)` retraso, ocioso, media | 2 771 us | **991 us** | 507 us |
| `nanosleep(1 ms)` retraso, ocioso, peor | 5 661 us | **1 761 us** | 1 836 us |
| `nanosleep(1 ms)` retraso, carga, media | 2 888 us | **975 us** | 828 us |
| `nanosleep(1 ms)` retraso, carga, peor | 10 356 us | **3 245 us** | 3 961 us |
| latencia de despertar carga/ocioso (media) | 1,04x | **0,98x** | 1,63x |

## 3.bis Comprobar una sospecha de regresión: A/B sobre un solo binario

Dos interruptores de línea de comandos devuelven el núcleo al comportamiento
anterior, para poder comparar **sin recompilar**:

```sh
# Comportamiento nuevo (por defecto)
./scripts/qemu-bench.sh -o /tmp/on.log "cd /root && /bin/eclipse-bench --quick --only sched ."

# Temporizador por plazo desactivado (caducidad solo en el tick de 250 Hz)
./scripts/qemu-bench.sh -c 'TIMERDEADLINE=0' -o /tmp/td0.log "..."

# Preempción por despertar desactivada
./scripts/qemu-bench.sh -c 'WAKEPREEMPT=0' -o /tmp/wp0.log "..."
```

Recompilar entre A y B **no** es una comparación: el binario y su disposición
cambian, y la varianza de una corrida bajo TCG basta para ocultar el efecto en
cualquier dirección. `cat /proc/perf/kernel` imprime `sched mode:` con el estado
de ambos interruptores, así que un log capturado dice en qué modo se produjo.

Resultado del A/B (misma máquina, mismo binario, solo cambia el arranque):

| | ambos ON | `TIMERDEADLINE=0` | `WAKEPREEMPT=0` |
| --- | ---: | ---: | ---: |
| `sleep 1 ms` ocioso, media | 878 us | 2 640 us | 934 us |
| `sleep 1 ms` **carga**, media | **1 396 us** | 5 766 us | **12 118 us** |
| pipe ida/vuelta **carga** | **2 545 us** | 1 463 us | **23 463 us** |
| latencia despertar carga/ocioso | **1,59x** | 2,18x | **13,0x** |

Es decir: el temporizador por plazo vale ~3-4x en latencia de `sleep`, y la
preempción por despertar vale **~9x** en el pipe bajo carga. Ninguno de los dos
degrada las cifras en reposo.

### Un fallo encontrado por este A/B

La primera corrida con **ambos** interruptores apagados no llegó a imprimir ni
una fila: se quedó colgada. La causa no era el código antiguo sino un fallo
introducido con la preempción por despertar. `WakerRef::wake_by_ref` filtraba los
despertares de tareas ya prestadas a un ejecutor (correcto: no hay nada que
desalojar) pero salía **sin enviar el IPI de entrega**. Bajo robo de trabajo el
dueño de la página de wakers no es la CPU que está ejecutando la tarea, así que
el despertar rediferido aterrizaba en la cola de una CPU que podía estar detenida
en `hlt` y sin nadie que la avisara — un despertar perdido, no solo latencia
extra. Con el temporizador por plazo activo los interrupciones frecuentes lo
tapaban; apagando ambos, se manifestó.

Corregido: un despertar de tarea en vuelo sigue enviando el IPI de entrega
(`maybe_send_resched_ipi`) aunque no publique petición de preempción.

## 3.ter El hallazgo estructural: `fork` no hace copy-on-write

`VMObjectPaged::fork_copy` (`zircon-object/src/vm/vmo/paged.rs:222`) recorre
**cada frame residente** del padre, asigna uno nuevo y hace `pmem_copy` de 4 KiB.
`fork` es por tanto O(memoria residente): un proceso con 100 MiB paga una copia
de 100 MiB *cada vez que se bifurca*. Linux comparte los frames y los protege
contra escritura; su `fork` apenas depende del tamaño del proceso.

Esto estaba oculto a plena vista. La fila `COW fault (after fork)` daba 578 ns
— quince veces *más rápida* que un fallo de página menor y 200x mejor que
Linux — porque con una copia eager las páginas del hijo ya son privadas: esa
fila estaba cronometrando escrituras normales, no fallos COW. Un número
absurdamente bueno era la señal.

`eclipse-bench` lo mide ahora directamente: bifurca con 1 MiB y con 16 MiB
residentes y publica la pendiente, comparada con lo que cuesta un `memcpy` de
1 MiB **en esa misma máquina** (para que el resultado no dependa del hardware).

| `--only proc`, mismo QEMU | Eclipse | Linux 6.8 |
| --- | ---: | ---: |
| `fork + exit`, 1 MiB residente | 7 617 us | 4 396 us |
| `fork + exit`, 16 MiB residente | **69 967 us** | 9 374 us |
| coste de `fork` por MiB residente | 4 157 us/MiB | 332 us/MiB |
| `memcpy` de 1 MiB en esa máquina | 993 us/MiB | 2 250 us/MiB |
| **ratio de copia en `fork`** | **4,19x** | **0,15x** |

Un ratio ≥ 1 significa que `fork` cuesta al menos un `memcpy` del proceso. El
4,19x de Eclipse dice que cuesta *cuatro veces más* que copiarlo: la copia, más
la asignación de un frame por página, más el montaje de las tablas. Linux, con
copy-on-write, está en 0,15x — el resto es copiar tablas de páginas y el
desmontaje del hijo al salir, que paga cualquier núcleo.

**Esta es la mayor mejora pendiente**, por encima de la vDSO: explica toda la
sección PROCESS (incluido `fork + exec`, 4,3x por detrás) y castiga
proporcionalmente a cada proceso grande del sistema. Las piezas ya existen en el
árbol — `VmObject::create_child` es el clon COW de Zircon, y `range_change` tiene
un `RangeChangeOp` documentado como «quitar el permiso de escritura para
Copy-on-Write» — pero `VmAddressRegion::fork_from` no las usa: llama a
`fork_copy`. No es un parche de una línea y toca la ruta más delicada del
núcleo, así que conviene abordarlo con la suite de medición ya montada delante.

## 3.quater Copy-on-write en `fork`

Implementado en `VmMapping::try_cow_child` (`zircon-object/src/vm/vmar.rs`).
`fork` entrega al hijo un clon snapshot del VMO de cada mapeo
(`VmObject::create_child`, el clon COW de Zircon: mueve los frames a un padre
oculto compartido, apunta a él a padre e hijo, y quita el permiso de escritura
de los mapeos existentes) en lugar de copiar cada frame residente.

Dos casos conservan a propósito el camino eager anterior: un VMO con **más de un
mapeador** (`create_child` desprotege *todos* los mapeos del objeto, lo que en un
mapeo genuinamente compartido convertiría las escrituras compartidas de otro
proceso en copias privadas) y todo lo que `create_child` rechaza (contiguo,
pinneado, no cacheado).

Además, tras `create_child` se vuelve a aplicar la desprotección de escritura
con lock bloqueante (`protect_for_cow`): `VmMapping::range_change` usa
`try_lock` y **se salta en silencio** un mapeo cuyo lock esté tomado en ese
instante — inofensivo para sus llamadores originales, pero aquí dejaría al padre
con PTEs escribibles sobre frames que el hijo comparte, es decir corrupción
silenciosa entre procesos. Un hilo hermano del proceso que bifurca fallando en
ese mismo mapeo es exactamente esa carrera.

### Lo medido

| `--only proc`, mismo QEMU | eager (antes) | **COW** | Linux 6.8 |
| --- | ---: | ---: | ---: |
| `fork + exit`, 16 MiB residente | 69 967 us | **22 821 us** | 9 374 us |
| coste por MiB residente | 4 157 us/MiB | **516 us/MiB** | 332 us/MiB |
| **ratio de copia en `fork`** | **4,19x** | **0,59x** | 0,15x |
| `fork + exec(/bin/sh -c :)` | 46 447 us | 39 946 us | 10 804 us |
| `COW fault (after fork)` | 578 ns (falso) | 89 512 ns (real) | 131 462 ns |
| `fork memory isolation` | — | **PASS** | — |

La fila de aislamiento es nueva y es la que importa: el padre llena una región
con un patrón, bifurca, el hijo la sobreescribe con otro y sale, y cada uno
comprueba que las escrituras del otro no le llegaron. Un `fork` que comparte
frames sin desprotegerlos es *rápido* y *incorrecto*; sin esa comprobación la
mejora no significaría nada.

Nótese también que la fila `COW fault` pasa de 578 ns (un número falso: sin COW
cronometraba escrituras normales) a 89,5 us — que además es **más rápido** que
los 131 us de Linux en el mismo emulador.

### Lo que el COW cuesta: `fork` pequeño 3x más caro

Medido después, con el A/B sobre un solo binario (`--only proc`, mismo QEMU):

| | COW (por defecto) | `FORKCOW=0` | |
| --- | ---: | ---: | --- |
| `fork + exit` | 10 956 us | **3 602 us** | eager **3,0x** |
| `fork + exit`, 1 MiB residente | 17 380 us | **5 276 us** | eager **3,3x** |
| `fork + exit`, 16 MiB residente | **23 641 us** | 41 986 us | COW **1,8x** |
| coste por MiB residente | **417 us/MiB** | 2 447 us/MiB | COW **5,9x** |
| ratio de copia | **0,57x** | 1,87x | COW |
| `fork + exec(self, estático)` | **9 630 us** | 11 705 us | COW **1,2x** |
| `fork + exec(/bin/sh -c :)` | **41 340 us** | 53 404 us | COW **1,3x** |

El cruce está entre 1 y 16 MiB residentes. Por debajo, la copia ansiosa gana: un
`memcpy` de unas pocas páginas bajo TCG es barato, mientras que el COW paga un
coste **fijo por mapeo** — `create_child` mueve los frames a un padre oculto y
`protect_for_cow` recorre el mapeo desprotegiendo PTEs y termina con un
`remote_flush_all()`. Es decir, **un shootdown de TLB completo por cada mapeo**,
y `fork_from` llama a `clone_map` mapeo a mapeo. Bajo TCG cada shootdown es una
ida y vuelta de IPI a las otras 3 CPUs con espera de acks; con muchos mapeos
pequeños eso domina sobre lo que se ahorra en copias.

**Se deja el COW activado por defecto igualmente**, porque las dos filas de
`fork + exec` —que es lo que hace un intérprete de órdenes de verdad, y la ruta
que el usuario nota al lanzar un comando— salen mejor con COW, y el coste por MiB
cae 5,9x. Lo que se degrada es `fork + exit`, un primitivo que casi ningún
programa real ejecuta desnudo.

El arreglo natural es agrupar los shootdowns: uno solo al final de todo el
`fork`, no uno por mapeo. **No es un cambio trivial y por eso no está hecho**:
entre desproteger las PTEs de un mapeo y vaciar el TLB hay una ventana en la que
un hilo hermano del proceso que bifurca puede escribir a través de una entrada de
TLB obsoleta sobre un frame que el hijo ya comparte — corrupción silenciosa entre
procesos. Hoy esa ventana dura las pocas instrucciones que hay dentro de
`protect_for_cow`; agrupando pasaría a durar todo el `fork`. Cerrarla necesita
razonar sobre qué CPUs pueden tener el espacio de direcciones activo, y merece su
propia pasada.

### El cuelgue: diagnosticado y corregido

Al principio esto se entregó **desactivado**, porque `fork` repetido sobre un
proceso con una región anónima ya paginada colgaba la máquina en dos de cada tres
corridas, sin pánico. Está resuelto.

**Dos defectos de observabilidad hacían el diagnóstico imposible**, y ambos
llevaron a una conclusión falsa («no salió el banner de deadlock, luego no es un
deadlock»):

1. `console_panic_banner` escribe **solo al framebuffer**, y su cuerpo x86_64
   entero está bajo `#[cfg(feature = "graphic")]` — que estas compilaciones de
   prueba no llevan. El detector de deadlocks era literalmente un no-op.
2. El umbral del detector es un **contador de giros** (1e9), documentado como
   «~8 s en hardware actual». Bajo QEMU/TCG el invitado va órdenes de magnitud
   más lento, así que ese contador tarda muchos minutos y el detector nunca
   llegaba a dispararse dentro de una corrida.

Corregidos: el informe se emite también por serie (`dl_paint`, zCore/src/lang.rs)
y el umbral es ajustable con `DEADLOCKSPINS=<n>` en la línea de comandos.

**La causa raíz: un auto-deadlock re-entrante en `Drop for VmObject`**
(`zircon-object/src/vm/vmo/mod.rs`). El destructor toma `parent.inner.lock()` y
lo mantiene hasta el final. Dentro del bucle, `child.upgrade()` produce un `Arc`
que se suelta al final del cuerpo; si resulta ser la **última** referencia, el
destructor de ese hijo corre *en línea, en la misma CPU, dentro de esa sección
crítica*, y su primer acto es volver a pedir `parent.inner.lock()`. `lock::Mutex`
es un ticket lock no reentrante, con interrupciones desactivadas y sin timeout:
gira para siempre, no suelta el cerrojo, y todo `fork` posterior se atasca detrás
(necesita ese mismo cerrojo vía `share_count`/`add_child`), también con las
interrupciones apagadas. Máquina parada, en silencio.

El guard `None => continue` que ya había se escribió para la carrera *contraria*
—un hermano ya dentro de su propio destructor, cuyo contador es 0 y falla el
`upgrade`— y mata correctamente la versión de dos CPUs. No puede matar esta:
en el momento del `upgrade` el contador todavía es >= 1; llega a cero después, y
en esta misma CPU.

**Era inalcanzable antes del COW**: `fork_copy` construía los hijos con
`VmObjectInner::default()` (sin padre), así que el destructor salía en el
`None => return` sin tomar ningún cerrojo. `create_child` es lo único que produce
un VMO con padre.

Corrección: aparcar todo `Arc` que se promueva en un `deferred` y dejarlos morir
sólo cuando ya no queda ninguna guarda tomada.

**Dos hipótesis descartadas empíricamente** antes de llegar ahí, con un
reproductor que traza el coste de cada iteración (`eclipse-bench --forkloop N MIB`):
60 forks a 1 MiB y 80 a 16 MiB salen **planos** (~1,5-2,9 ms y ~6-9 ms), sin
tendencia alguna — o sea, el árbol COW sí colapsa y no hay acumulación por fork.
Esa traza distingue en un solo arranque un fallo algorítmico (curva que se
dispara) de un atasco (plano y de pronto nada).

**Validación tras la corrección**: 7 corridas de la suite `--only proc` que antes
fallaba 2 de cada 3, más 270 forks trazados, en tres arranques. Cero cuelgues,
cero informes de deadlock, `fork memory isolation` PASS, y la shell sigue viva.

### Otros defectos que destapó el análisis

- **`protect_for_cow` iteraba `size / PAGE_SIZE` (redondeo hacia abajo)** mientras
  que `flags` se dimensiona con `pages(size)` (hacia arriba). En un mapeo cuyo
  tamaño no es múltiplo de página eso dejaba la última página escribible sobre un
  frame ya compartido — exactamente la corrupción que la función existe para
  evitar. Corregido a `inner.flags.len()`. (`map_committed` tiene la misma
  costumbre de redondear hacia abajo; es preexistente y no se ha tocado.)
- **`VmObject::remove_mapping` no tiene ningún llamador** (`impl Drop for
  VmMapping` sólo llama a `unmap()`), así que `mapping_count` nunca decrementa y
  `share_count()` es una marca de máximo histórico, no un recuento vivo. El
  efecto sobre el guard `share_count() > 1` del COW es *conservador* (desactiva
  COW de más, nunca de menos), así que no es un riesgo de corrección — pero
  degrada la cobertura en silencio. Sin corregir: hacer el recuento exacto haría
  que COW se aplicase en **más** casos, que es la dirección arriesgada.
- **El benchmark calculaba `f1` y `f16` antes de imprimir ninguna de las dos
  filas**, así que «no salió la fila de 1 MiB» no implicaba que el cuelgue
  estuviese en el bucle de 1 MiB — podía estar igualmente en el de 16 MiB. La
  descripción del síntoma con la que empecé era imprecisa por esto.

## 3.quinquies La vDSO: `clock_gettime` sale del núcleo

Era la mayor brecha que quedaba y la única de más de un orden de magnitud. No se
cerraba abaratando el trap: el nuestro ya era unas tres veces más barato que el
de Linux (`getpid` 7 422 ns contra 14 746 ns en la misma sesión). Linux
sencillamente **no lo toma**. Mapea una pequeña biblioteca en cada proceso y
responde en espacio de usuario leyendo el TSC.

Ahora Eclipse también.

### Lo medido

Un solo binario, dos arranques, mismo QEMU/TCG con 4 vCPU:

| `clock_gettime(MONOTONIC)` | | |
| --- | ---: | --- |
| Eclipse, vDSO inactiva | 7 577 ns | |
| Eclipse, vDSO activa | **152,0 ns** | **50x** más rápido |
| Linux 6.8 (control, misma sesión) | 145,9 ns | empate |

Confirmado con el binario final en corridas limpias: sin vDSO 8 270 ns
(`getpid` 8 165 ns en la misma corrida — es decir, un syscall y nada más), con
vDSO 141,6 ns. Ahí el banco además informa `vDSO: absent (AT_SYSINFO_EHDR not
published)`, que es lo correcto: cuando el reloj no se puede servir en espacio de
usuario no se anuncia nada, así la libc no paga una llamada indirecta antes de
cada lectura a cambio de nada.

El resto de la sección `SYSCALL` no se mueve más allá del ruido de TCG, que es
lo que se espera de un cambio dirigido a una sola ruta.

### Por qué el peor caso es "sin aceleración" y nunca una hora incorrecta

musl trata cualquier error salvo `-EINVAL` como «la vDSO no ha sabido» y cae al
syscall (`src/time/clock_gettime.c`: *"Fall through on errors other than
EINVAL"*). Así que todo lo que la imagen no sirve —el reloj deshabilitado por el
núcleo, un `clock id` que no implementa— devuelve `-ENOSYS`. Un fallo aquí cuesta
rendimiento, no corrección.

Esa propiedad es la que permitió desplegarla: la parte difícil de una vDSO no es
el cálculo, son los metadatos ELF, y **todo lo que puede salir mal ahí sale mal
en silencio**. Sin `DT_HASH` musl no encuentra símbolos; con dos `PT_LOAD`
calcula mal el sesgo de carga; con una reubicación dinámica se desreferencia una
entrada GOT que nadie rellena. Nada de eso falla al enlazar. Por eso
`linux-vdso/build.rs` recorre el ELF resultante y ejecuta contra él **el propio
algoritmo de musl** (`__vdsosym`); si `__vdso_clock_gettime` no resuelve, la
compilación para. Y `linux-vdso/tests/execute.rs` mapea la imagen como lo hará el
núcleo y la **llama de verdad** en el host: la multiplicación de 128 bits con un
producto que desborda 64, el reloj sin retrocesos, los caminos no servidos
devolviendo `-ENOSYS` y no `-EINVAL`, y la misma imagen desde ocho direcciones de
carga distintas. Ambas comprobaciones tardan un segundo; el ciclo equivalente
dentro de QEMU son cuarenta minutos.

### `VDSOFORCE=1`: por qué existe

La vDSO solo se activa si CPUID declara el TSC **invariante** (ritmo constante y
sincronizado entre núcleos). Es el mismo criterio que hace que `timer_now`
prescinda del suelo monotónico: si el núcleo no se fía del contador, el espacio
de usuario tampoco debe, porque un hilo que migre podría ver la hora retroceder y
—a diferencia del núcleo— no puede participar en ese suelo.

El problema es de medición: **QEMU no puede anunciar `invtsc` bajo TCG**. Esa
palabra de características no está en su conjunto soportado, así que `+invtsc` en
la línea de órdenes se descarta en silencio. Y TCG es el único sustrato donde
Eclipse y Linux se comparan en igualdad. De ahí el interruptor:

```sh
./scripts/qemu-bench.sh -c 'VDSOFORCE=1' -o /tmp/on.log "eclipse-bench --only syscall"
```

Es sólido bajo TCG, donde el `rdtsc` de cada vCPU sale de un único reloj
anfitrión y por tanto está sincronizado por construcción. **No** es un
interruptor para hardware real que se niegue a declarar el TSC invariante.

En la práctica nadie lo necesita fuera del banco de pruebas: `make run ACCEL=1`
ya arranca con `-cpu host,migratable=no,+invtsc`, y sobre KVM o sobre metal la
característica está presente, así que la vDSO se activa sola.

Queda pendiente —y es lo que haría innecesario el interruptor— **verificar la
sincronización del TSC empíricamente** en el arranque, como hace Linux, en vez de
fiarlo todo a CPUID. El núcleo ya tiene media pieza: `mono_floor_tick` degrada a
la ruta con suelo si observa desviación. Falta el sentido contrario, promover
tras un arranque sin desviación observada.

### Diagnóstico

Todas las formas en que esto puede no activarse son silenciosas —el invitado
sigue funcionando, el reloj solo sigue siendo caro— así que ambos lados lo dicen:

```
$ cat /proc/perf/kernel
vdso:         activa, tsc_mult=1530091662 (0.356 ns/tick)

$ eclipse-bench --only syscall
  vDSO: mapped at 0x7ffffff7c000
```

Son preguntas distintas a propósito. La del núcleo dice si hay imagen, si se pudo
instalar y si el reloj está publicado. La del banco dice si la libc llegó a
recibir `AT_SYSINFO_EHDR` — lo que separa «el núcleo no ofreció nada» de «la libc
miró y declinó». Adivinar entre las dos a partir de un tiempo cuesta un ciclo de
arranque en cada dirección.

## 3.sexies Dos bugs de estabilidad, cazados por el banco ampliado

Al ampliar el banco (hilos, futex, señales, socketpair, y el `fork` por número
de mapeos bajo carga) empezó a colgarse la máquina: aproximadamente una de cada
dos rondas de la sección SCHEDULER. Resultaron ser **dos bugs distintos**,
ambos anteriores a esta sesión, que las cargas nuevas destaparon.

### El despertar perdido del pipe

Ambos extremos de un ping-pong dormidos para siempre, con la máquina en idle
sano alrededor (vCPUs en `hlt` con IF=1, ticks entregándose, colas vacías).
`PipeFuture::poll` comprobaba disponibilidad y *después* volvía a tomar el
cerrojo para suscribirse al `EventBus`; si el escritor colaba su byte entre
ambas tomas, disparaba a los suscriptores de ese instante — ninguno. Y la
semántica del bus lo convertía en permanente: los callbacks disparan solo en
**transiciones** de flags y los flags quedan memorizados, así que un segundo
`set(READABLE)` sobre un READABLE ya puesto no despierta a nadie; quien
limpiaría el flag es exactamente el lector dormido.

Arreglo en dos capas: `EventBus::subscribe` dispara inmediatamente si hay
eventos activos al suscribirse (cierra la clase para pty, sockets unix, stdio,
input y semáforos, que usan el mismo patrón), y `PipeFuture::poll` evalúa y se
suscribe bajo una sola toma del cerrojo.

### El autointerbloqueo del árbol COW

Tras arreglar el pipe, la cuña cambió de morfología: de silenciosa a banner
girante — `cpu=N` esperándose **a sí misma** en el cerrojo de familia de un
VMO. La cadena: `Drop(Snapshot)` → `remove_child` en el nodo oculto →
`replace_child` en el **abuelo**, que mejora al otro hijo del abuelo y clona
dos `Arc` por nivel en su bucle de propagación de owners — y dejaba morir todos
esos `Arc` en su ámbito, bajo el cerrojo. Con un teardown concurrente soltando
la última otra referencia (los `fork+exit` del banco con hogs muriendo a
SIGKILL fabrican esa carrera), el drop en ámbito era el último, `Drop`
reentraba en la CPU que ya tenía el cerrojo, y el ticket lock — no reentrante —
giraba para siempre.

El arreglo generaliza una invariante que el árbol ya conocía a medias (`set_len`
difería su `Arc` con un comentario explicándolo): **ningún `Arc` de la familia
muere bajo el cerrojo de familia**. Todo lo que `remove_child` y
`replace_child` mejoran o clonan viaja en un `Vec` de diferidos que se vacía en
el marco exterior con el cerrojo liberado. Validado con 8/8 rondas limpias de
la sección que mataba una de cada dos.

### Los instrumentos, que se quedan

Ninguna de las siete derivaciones estáticas que se intentaron encontró el sitio;
lo encontró una cascada de instrumentos, y por eso se quedan en el árbol:

1. **El espejo serial del banner reemite al ganar contenido** — antes, un
   candado de una sola vez enviaba la línea del waiter y se tragaba la del
   `HOLDER`, que es la que nombra al culpable.
2. **`track_caller` en `get_inner`/`get_inner_mut`** — los banners nombran la
   función real (`Drop`, `commit_page`, …), no el vestíbulo genérico.
3. **Detector instantáneo de reentrada en el ticket lock** — «el holder soy yo»
   nunca es contención; se reporta en el acto (con relectura a 130k giros para
   inmunizarlo contra registros rancios) en vez de tras el umbral de segundos.
4. **`VMO-DROP-REENTRY` con migas de pan** — un contador de profundidad por CPU
   en el propio `Drop` imprime el par de objetos anidados y una máscara de bits
   del camino recorrido, antes de que la máquina muera. La máscara `0x27` fue
   la que redujo el problema a una ventana de diez líneas.
5. **El vigilante del arnés** — si el log de QEMU se congela con el guest vivo,
   interroga las vCPUs por el monitor (`info registers`, `info lapic`) y deja
   el volcado en el log. Distinguió «máquina sorda» de «máquina vacía», que era
   la bifurcación clave del diagnóstico.

Dos lecciones de método quedaron pagadas con horas: los símbolos bajo LTO
mienten (identical-code-folding funde funciones idénticas y el nombre mostrado
es el representante — el «TicketMutex de ItimerSlot» era el cerrojo de
familia), y `pgrep -c qemu-system-x86_64` devuelve 0 siempre porque el patrón
supera los 15 caracteres del nombre de proceso.

## 3.septies SMP: los caminos del kernel, medidos contra Linux

La sección SMP nueva (getpid xN, shootdown aislado, mmap/fallos paralelos,
mutex contendido, pipe fijado por afinidad, forks paralelos, equidad) corrió en
ambos kernels bajo el mismo QEMU. Cada fila xN lleva su línea base x1, así que
los escalados sobreviven a la emulación.

| fila | Eclipse | Linux 6.8 | lectura |
| --- | ---: | ---: | --- |
| escalado ALU x4 (colocación) | 102,9 % | 81,9 % | Eclipse reparte perfecto |
| `getpid` x1 (absoluto) | 0,15 Mops/s | 0,07 Mops/s | Eclipse 2x |
| escalado de syscall x4 | 72,5 % | 96,9 % | contención en la entrada de Eclipse |
| `mprotect`, vecinos ociosos | 79 us | 44 us | conocido (1,8x) |
| **coste de shootdown** (girando/ocioso) | **11,8x** | 4,45x | la invalidación remota cuesta 2,7x lo que a Linux *bajo el mismo emulador* |
| **`mmap` x4 vs x1** | **0,05 %** | 10,9 % | colapso total del cerrojo de VM |
| **fallos menores x4 vs x1** | **0,30 %** (54,9 → 0,66 kflt/s) | 41,2 % | patológico: 4 hilos fallando son **80x más lentos en absoluto** que 1 |
| colapso de mutex contendido | 5,8x | 4,4x | futex comparable |
| pipe fijado misma-CPU | 60,6 us | 211 us | Eclipse 3,5x |
| **despertar cross-CPU (fijado)** | **122x** (60 us → 7,4 ms) | 1,25x | el despertar remoto de un hilo fijado cae a granularidad de tick (7,4 ms ≈ 2 ticks de 4 ms) |
| forks/s x1 / x4 | n/a | 383 / 1 629 | la sonda destapó un bug de ABI (abajo) |
| equidad max/min | n/a | 1,63x | ídem |

### Lo que la batería encontró, por orden de valor

1. **El despertar cross-CPU de un hilo fijado cuesta 7,4 ms — dos ticks.** Con
   ambos extremos del pipe fijados en CPUs distintas (afinidad real, no robo de
   trabajo que los junte), el despertar no llega por IPI: llega cuando el tick
   del receptor mira su cola. La preempción por despertar de esta sesión no
   alcanza a los objetivos fijados en otra CPU. Es la explicación más probable
   de la cola de latencias `worst` bajo carga (12-15 ms) que el banco venía
   registrando.
2. **Las operaciones de VM paralelas colapsan a ~0 %.** No es una cola de
   cerrojo (eso daría ~25 % con 4 hilos): 4 hilos fallando páginas rinden 80x
   MENOS en absoluto que 1. Huele a convoy del ticket lock con IRQs
   deshabilitadas más shootdowns interfiriendo entre sí — cada spin con IRQs
   off impide ackear las invalidaciones de los demás.
3. **El shootdown aislado: 11,8x contra 4,45x de Linux.** El listón justo es el
   4,45x (las IPIs bajo TCG son caras para ambos); el exceso de Eclipse es la
   espera de acks de `remote_flush_tlb`.
4. **`MAP_SHARED | MAP_ANONYMOUS` no se comparte a través de `fork`.** Las dos
   sondas que usan una página compartida padre-hijo salen n/a en Eclipse y
   funcionan en Linux: el `fork` de Eclipse copia también los mapeos
   compartidos (el eager fallback trata todo como privado). Es un bug de
   corrección del ABI, no de rendimiento, y cualquier programa que use memoria
   compartida anónima entre procesos (nginx, postgres, cualquier pool
   preforkeado) está afectado.
5. **Victorias:** colocación perfecta, syscalls absolutas 2x, pipe misma-CPU
   3,5x, futex a la par.

## 3.octies Arreglos SMP: dos resueltos, dos mejorados, uno nuevo

Los cuatro hallazgos de la seccion 3.septies se atacaron; medido en el kernel
tras integrar master (COW fork queda OFF por defecto, decision de master por una
corrupcion aparte — ver README-memory-leaks.md).

| fila | antes | despues | Linux | estado |
| --- | ---: | ---: | ---: | --- |
| despertar cross-CPU (fijado) | 122x (7,4 ms) | **7,86x (548 us)** | 1,25x | **resuelto**: ya no espera al tick |
| `forks/s` / `fairness` | n/a | 181 / 4,46x | 383 / 1,63x | **resuelto** (probe corre) + hallazgo |
| escalado de fallos x4 | 0,30 % | 12,6 % | 41 % | mejorado 42x |
| `mmap` x4 vs x1 | 0,05 % | 2,91 % | 11 % | mejorado 58x |
| coste de shootdown | 11,8x | 7,19x | 4,45x | mejorado |

**Resueltos.**

- *Despertar cross-CPU (bug 1).* El escaneo que saltaba una tarea notificada por
  afinidad la re-armaba sin avisar a nadie; el siguiente escaneo de un CPU
  dormido es su tick (7,4 ms = dos ticks de 4 ms). Ahora `kick_for_affinity`
  reenvia el despertar a un CPU permitido — dormidos primero — por la
  coalescencia de `request_resched`. 7,4 ms -> 548 us, 13,5x. Lo que queda
  (7,86x sobre same-CPU) es el coste de la IPI bajo TCG, no la latencia de tick.
- *MAP_SHARED sobrevive al fork (bug 4).* Marcador `share_on_fork` en el VMO,
  puesto en mmap anonimo/fichero compartido y en shm SysV; `clone_map` comparte
  el Arc igual que con los VMO fisicos. Las dos sondas que salian n/a (usan una
  pagina compartida padre-hijo) ahora corren, lo que ademas destapo el hallazgo
  de abajo.

**Mejorados, con residuo arquitectonico.**

- *Convoy de VM paralelo (bug 2).* La bomba de acks (`set_spin_pump`: un CPU
  girando con IRQs off drena su propia cola de shootdowns) subio el escalado de
  fallos del 0,30 % al 12,6 % y el de mmap del 0,05 % al 2,91 % — 42x y 58x. El
  techo que queda es el cerrojo unico del espacio de direcciones (el `inner` del
  VMAR), que serializa a los mapeadores como el viejo `mmap_sem` de Linux;
  cerrarlo del todo necesita cerrojos por-rango o por-VMA, un cambio mayor.
- *Coste de shootdown (bug 3).* Filtrar los objetivos por espacio de direcciones
  activo (cada CPU publica su raiz de tabla antes de escribir CR3) lo bajo de
  11,8x a 7,19x; Linux paga 4,45x en el mismo emulador. El resto es la espera de
  acks bajo TCG, donde cada IPI es cara para ambos.

**Hallazgo nuevo: injusticia del planificador, 4,46x.** Con `2N` hogs
identicos peleando por `N` CPUs, el que mas progresa hace 4,46x lo del que
menos (Linux: 1,63x). Solo es visible ahora: la sonda cuenta el progreso de
cada hog en su linea de cache de una pagina *compartida*, que antes del arreglo
del bug 4 el fork privatizaba — asi que la injusticia estaba ahi todo el tiempo,
tapada por otro bug. Es la explicacion mas probable de las colas de latencia
`worst` bajo carga. Queda anotado para investigar.

## 3.nonies btrfs sobre AHCI: Eclipse contra Linux en disco real

La fila DISK del banco corria en el cwd del shell — una raiz en RAM en ambos
kernels, o sea la cache de paginas, no un filesystem sobre un dispositivo. El
harness nuevo (`scripts/qemu-btrfs-bench.sh`) adjunta una imagen btrfs identica
a los dos kernels por el MISMO controlador ich9-ahci, la monta y mide alli.
Imagen de 1 GiB formateada con el layout que la crate btrfs in-tree de Eclipse
emite (`-O ^free-space-tree`, crc32c, nodos de 16K), working set de 128 MiB,
copia fresca por kernel. Ambos montan `/dev/sda` btrfs y escriben.

| metrica | Eclipse | Linux 6.8 | |
| --- | ---: | ---: | --- |
| escritura secuencial (+fsync) | 7,7 MB/s | 44,0 MB/s | Linux **5,7x** |
| **lectura secuencial** | **2,8 MB/s** | 379 MB/s | Linux **135x** |
| **lectura aleatoria 4K** | **35 IOPS** | 29 132 IOPS | Linux **828x** |
| latencia lectura 4K | **28 380 us** | 34 us | Linux 828x |
| latencia fsync (mejor) | 6,43 ms | 6,79 ms | empate |
| crear ficheros pequenos | 134 files/s | 1 129 files/s | Linux 8,4x |
| stat | 6 288 stats/s | 22 554 stats/s | Linux 3,6x |
| unlink | 349 unlinks/s | 1 681 unlinks/s | Linux 4,8x |

El desastre es la **lectura**: 28 ms por lectura aleatoria de 4K. No es un fallo
del filesystem — es la ruta de I/O de bloque, y son tres cosas multiplicandose,
las tres ausentes en Linux:

1. **AHCI de profundidad de cola 1, por sondeo.** `rw_block` usa siempre el
   slot 0 y `exec_cmd` hace busy-wait hasta que el comando termina antes de
   emitir el siguiente: un comando en vuelo, cero solapamiento. Linux usa NCQ
   con hasta 32 comandos en vuelo e interrupciones, asi que decenas de lecturas
   avanzan a la vez.
2. **Cache de bloque de 8 MiB contra un working set de 128 MiB.**
   `CACHE_HOT_CAP` deja ~8 MiB por dispositivo; con 128 MiB de datos el 94 % de
   las lecturas aleatorias fallan y van al disco. Linux cachea en la page cache,
   dimensionada a la RAM (4 GiB aqui).
3. **Amplificacion de metadatos de btrfs.** Una lectura logica de 4K recorre el
   B-tree de extents y el de fs; cada nodo son 16K y, con la cache demasiado
   pequena para retenerlos, cada lectura logica dispara varias lecturas fisicas
   — cada una a profundidad 1 y por sondeo (punto 1).

La escritura sufre menos (5,7x) porque se agrupa y se acolcha antes del `fsync`,
y la latencia de `fsync` empata: el coste esta en el volumen de lecturas
concurrentes, no en la barrera de durabilidad.

Es un hallazgo grande y accionable, pero el arreglo es trabajo de otra escala,
no un parche. Por orden de impacto/coste:

- **Cache de bloque mas grande** (subir `CACHE_HOT_CAP`) es el cambio de una
  constante y cubriria el working set — palanca barata, con el coste de RAM que
  toque medir.
- **AHCI con NCQ + interrupciones** (varios comandos en vuelo, sin busy-wait) es
  lo que cierra los 135-828x de lectura, y es un rediseno del driver.
- **Readahead secuencial** en la capa de bloque ayudaria a la fila de lectura
  secuencial sin tocar el driver.

El banco queda montado y verificado en ambas direcciones, asi que cualquiera de
esos cambios se mide con `scripts/qemu-btrfs-bench.sh` de una pasada.

## 3.decies btrfs sobre AHCI: el arreglo

El hallazgo de 3.nonies (lectura desastrosa) resulto no ser el driver. Una sola
medida lo redirigio: `dd` crudo sobre `/dev/sda` da 108 MB/s en bloques de 1 MiB
y 22 MB/s en 4K — el AHCI y la capa de bloque son rapidos. El desastre estaba en
la politica de cache/readahead de `CachedDevice`, la capa que btrfs atraviesa.

Dos fallos, los dos corregidos y medidos:

1. **Readahead incondicional.** La ventana de prefetch se aplicaba a toda
   lectura menor que ella, aleatoria incluida. Cada lectura de 4K arrastraba
   1 MiB (256x de mas) y expulsaba justo los nodos de btree que la siguiente
   lectura volvia a pedir: thrash puro. Ahora el readahead solo se dispara en
   **continuacion byte-exacta** de la lectura anterior. Un stream secuencial
   colapsa en un comando grande por ventana; un paseo aleatorio pide solo sus
   4K y deja el cache lleno de los metadatos que reusara.
2. **Cache de 8 MiB** contra working sets mucho mayores, subido a **64 MiB**.

| metrica | antes | despues | vs Linux antes | vs Linux despues |
| --- | ---: | ---: | ---: | ---: |
| **lectura aleatoria 4K** | 35 IOPS | **1 634-4 472 IOPS** | 828x | **22x** |
| latencia aleatoria 4K | 28 380 us | 224-612 us | 828x | 22x |
| escritura secuencial | 7,7 MB/s | 14,7-31,9 MB/s | 5,7x | 2,0x |
| lectura secuencial | 2,8 MB/s | 6,4-10,3 MB/s | 135x | 62x |
| fsync (mejor) | 6,43 ms | 1,72-6,5 ms | empate | empate |

(El rango es variancia de TCG entre corridas; una pasada aislada da el extremo
alto, una tras contencion del anfitrion el bajo.)

El cambio de mayor palanca fue el readahead adaptativo: la lectura aleatoria de
4K, la peor metrica, paso de 828x por detras de Linux a 22x. Se probo y descarto
POR MEDIDA una deteccion mas floja de secuencialidad ("hacia delante dentro de
una ventana"): hundia el aleatorio 10x sin recuperar el secuencial, porque btrfs
si mete lecturas aleatorias dentro de una ventana. `dd` crudo separo driver de
filesystem y evito un rediseno del driver que habria sido el instinto erroneo.

**Queda abierta la lectura secuencial** (62x tras Linux): los metadatos
intercalados de btrfs rompen la cadena secuencial byte-exacta, asi que muchas
lecturas de datos no disparan el readahead. Cerrarlo necesita estado de
readahead por-stream (un readahead que sobreviva a un desvio corto a un nodo de
btree), no una constante — trabajo de otra escala, anotado.

## 3.undecies Fork O(n²): dos causas cuadráticas, y el `fork` por mapeo que bate a Linux

El `forkloop` (traza por iteración de `fork` y de `wait`) mostró que un `fork`
de un proceso con M mapeos escalaba de forma **cuadrática**: 64 mapeos costaban
16 ms, 128 → 55 ms, 256 → 200 ms, 512 → 800 ms, y 1024 **fallaba** el `fork`.
~4x por cada duplicación de M es la firma de un O(M²). Separando `fork` de
`wait` en la traza salieron **dos** O(M²) distintos, en dos capas distintas.

**Causa 1 — el desmontaje del hijo (`wait`), en el allocator.** El `dealloc` del
buddy (`buddy_system_allocator` 0.8) fusiona bloques **escaneando**
`free_list[class]` en busca del hermano — O(longitud de la lista). Un `fork`
asigna y libera del orden de M objetos del mismo tamaño (el inner de
`VMObjectPaged`, `VmMapping`, `VmMappingInner`) cuyos hermanos siguen vivos, así
que esas listas crecen y cada `dealloc` paga O(M): el desmontaje era O(M²).
HEAPPROF lo confirmó (dealloc de ~18 K a ~284 K ciclos/llamada a lo largo de un
bucle de 512 mapeos).

Se antepone al buddy una **cache de free-lists por clase de tamaño** (el puntero
`next` vive en la primera palabra del bloque liberado, como la lista intrusiva
del propio buddy) que sirve el par asignar/liberar pequeño en O(1). Se apoya en
un invariante del buddy: todo bloque de clase c está alineado a 2^c, así que como
`class_size ≥ align` cualquier bloque cacheado de la clase satisface la
alineación de cualquier asignación que caiga en ella. Cachea 8 B..4 KiB (todo
objeto caliente de `fork`); tope de 1024 bloques/clase (~8 MiB del heap de
512 MiB). Solo en la build por defecto; `mem-debug` sigue yendo al buddy para sus
canarios. `zCore/src/memory_x86_64.rs`.

**Causa 2 — el montaje COW del padre (`fork`), en la capa VM.** Un `mprotect` o
un `munmap` que perfora un agujero PARTE el mapeo pero la cola conserva el MISMO
Arc de VMO (`cut`: `vmo: self.vmo.clone()`), así que un proceso puede tener M
mapeos sobre **un solo** VMO. `clone_map` clonaba cada mapeo por separado, y para
cada uno `create_child` recorre **todos** los mapeadores del VMO poniendo
`RemoveWrite` (`paged.rs:1343`); peor aún, `try_cow_child` rechaza
`share_count > 1` — justo ese caso — y los mandaba a **copia eager del VMO
entero, una por mapeo**. En ambas ramas, M mapeos sobre un VMO cuestan O(M²).

Se **deduplica el hijo por koid del VMO** dentro de `fork_from`: se genera el
hijo (snapshot COW o copia eager) una vez por VMO único y todos los mapeos
hermanos se mapean sobre ese mismo hijo. El primer `create_child` ya protegió los
PTE de todos los hermanos en su recorrido, así que el resto solo necesitan un
`VmMapping` nuevo. Que los hermanos compartan el hijo además preserva a través del
`fork` el aliasing que los mapeos tenían en el padre (antes divergían en copias
separadas — un bug latente de aliasing que esto también corrige).
`zircon-object/src/vm/vmar.rs`.

### Lo medido (mismo QEMU/TCG, 4 vCPU)

Escalado del `fork` (tiempos ya calientes), antes y después de las dos
correcciones:

| mapeos | antes | ahora | factor |
|-------:|------:|------:|-------:|
|     64 |   ~16 ms | 3,7 ms |    4x |
|    128 |   ~55 ms | 4,4 ms |   12x |
|    256 |  ~200 ms | 5,6 ms |   36x |
|    512 |  ~800 ms | 8,0 ms |  100x |
|   1024 | fallaba | 13,0 ms |    -- |

El escalado pasa de ~4x por duplicación (O(M²)) a ~1,2-1,6x (lineal). Y contra
Linux bajo el MISMO emulador, la batería `--only vm/proc`:

| métrica | Eclipse | Linux (TCG) | veredicto |
|---|---:|---:|---|
| COW fault (tras fork) | 457 ns | 106 892 ns | **Eclipse 234x** |
| minor fault (anon) | 12 570 ns | 68 959 ns | **Eclipse 5,5x** |
| mmap+munmap (4 KiB) | 45 680 ns | 129 748 ns | **Eclipse 2,8x** |
| mprotect (4 KiB, x2) | 27 809 ns | 77 436 ns | **Eclipse 2,8x** |
| **fork coste por mapeo** | **8,68 us** | **22,3 us** | **Eclipse 2,6x** |
| fork+exit, 256 mapeos | 7 436 us | 8 635 us | **Eclipse 1,16x** |
| fork + exit (base) | 2 621 us | 2 704 us | empate |
| fork coste por MiB residente | 1 287 us | 167 us | Linux 7,7x |
| fork + exec(/bin/sh -c :) | 40 249 us | 7 619 us | Linux 5,3x |

`fork memory isolation` sigue en **PASS** en ambos: la deduplicación no rompe el
COW. El camino que hacía cuadrático un `fork` con muchos mapeos —el más
catastrófico, 800 ms a 512 mapeos— ahora **bate a Linux** (2,6x por mapeo).

(Nota: las cifras de arriba se midieron con COW **desactivado**, que era el valor
por defecto en ese momento; la fila «COW fault» de 457 ns es en realidad un
minor-fault sobre la copia eager, no un COW real. Con COW por defecto —abajo—
cambian.)

### COW por defecto: la «corrupción» era una entrada de TLB obsoleta

El `fork` copiaba **eagermente** (COW desactivado) porque el copy-on-write se
había revertido dos veces, la última por corrupción de memoria de usuario con un
reproductor determinista (`dd if=/dev/zero of=/tmp/z bs=4096 count=64 &&
md5sum /tmp/z` → `malloc(): invalid next size`, SIGABRT, siempre): glibc leía una
cabecera de trozo que no había escrito, es decir **dos procesos escribiendo una
página que debía copiarse**.

La causa no estaba en la lógica COW, sino en una **entrada de TLB escribible
obsoleta**. `protect_for_cow` protege contra escritura las páginas del padre, y el
shootdown debe invalidar el TLB de las demás CPU; pero una CPU girando en un
ticket lock tiene las IRQs deshabilitadas, no podía atender el IPI de shootdown, y
el iniciador quemaba su presupuesto de acks y se rendía. Esa CPU conservaba una
entrada escribible hacia un marco que el hijo ya compartía, y una escritura por
ella caía sobre la página del hijo — exactamente la corrupción. Es la misma raíz
que el hallazgo SMP #3 (el convoy de VM): el **spin-pump** de shootdowns
(`tlb_shootdown_pump`, drenado cada 512 vueltas del ticket lock) hace que una CPU
que gira vacíe su propia cola, así que ninguna entrada escribible sobrevive al
write-protect.

Revalidado (FORKCOW=1, 4 vCPU): el reproductor exacto y variantes más duras —20x
md5 en serie, un bucle sobre un fichero aleatorio de 2 MiB, **40x md5 en
paralelo** (el camino SMP con más probabilidad de destapar la carrera)— devuelven
todos un único checksum idéntico; `fork memory isolation` da PASS; el shell de
login sobrevive. **COW queda activado por defecto**; `FORKCOW=0` es el
kill-switch. La build por defecto, contra Linux bajo el mismo QEMU/TCG:

| métrica | Eclipse (COW) | Linux (TCG) | veredicto |
|---|---:|---:|---|
| COW fault (tras fork) | 19 158 ns | 106 892 ns | **Eclipse 5,6x** |
| fork + exit (base) | 1 366 us | 2 704 us | **Eclipse 2,0x** |
| fork + exit, 1 MiB | 1 521 us | 2 945 us | **Eclipse 1,9x** |
| **fork coste por mapeo** | **4,57 us** | 22,3 us | **Eclipse 4,9x** |
| fork + exit, 256 mapeos | 2 734 us | 8 635 us | **Eclipse 3,2x** |
| fork + exit, 16 MiB | 7 701 us | 5 451 us | Linux 1,4x (era 4,2x) |
| fork coste por MiB residente | 412 us | 167 us | Linux 2,5x (era 7,7x) |
| fork + exec(/bin/sh -c :) | 45 290 us | 7 619 us | Linux 5,9x |

Activar COW bate a Linux en el `fork` base, el de 1 MiB, el coste por mapeo (4,9x)
y el COW fault (5,6x), y estrecha el residente de 7,7x a 2,5x y el de 16 MiB de
4,2x a 1,4x. **Queda abierto** el residente (que Linux aún gana 2,5x: su hijo deja
los PTE vacíos y pagina bajo demanda, Eclipse aún instala los PTE comprometidos en
`map_committed`) y `fork + exec` de binario grande (5,9x, el copiado del segmento
ELF en `make_vmo`, ya anotado en §5.3). Ambos son «poblado perezoso»: mover el
coste del `fork`/`exec` al primer acceso, trabajo de otra escala.

## 4. Correcciones aplicadas

### 4.1 Preempción por despertar (`vendor/PreemptiveScheduler`)

El ejecutor sondea una tarea hasta que su futuro devuelve `Pending`, y un hilo de
usuario ligado a CPU solo lo hace al expirar su rodaja (20 ms). Despertar una
tarea solo ponía un bit en la página de wakers de su CPU, y el IPI de
replanificación se enviaba **únicamente a CPUs detenidas en `hlt`**. Una tarea
despertada sobre una CPU *ocupada* esperaba la rodaja completa de la otra.

Ahora `NEED_RESCHED` (máscara por CPU) publica la petición, el IPI se manda
también a CPUs ocupadas, y `handle_user_trap` la consume en *cualquier* vector
de interrupción y cede. Se filtran los despertares de tareas ya prestadas a un
ejecutor (que es lo que hace `yield_now` consigo misma) y las peticiones se
fusionan: una ráfaga cuesta un IPI, no N. `spawn_task` usa el mismo camino, así
que un hijo recién bifurcado no espera 20 ms a su primera instrucción.

### 4.2 Balanceo por tareas ejecutables, no totales

`task_num()` cuenta **todas** las tareas, incluidas las aparcadas en `Pending`, y
era el número que usaban la colocación y el robo de trabajo. Una CPU con
cincuenta demonios dormidos parecía más cargada que una con dos hilos girando a
tope. `TaskCollection::ready_num()` cuenta solo `notified & !dropped & !borrowed`
y ambos caminos la usan.

### 4.3 Temporizador programado por plazo (`kernel-hal/src/bare/timer.rs`)

Los temporizadores caducaban **solo** en el tick periódico de 250 Hz:
`timer_set` empujaba al montículo y `timer_tick` drenaba lo vencido. Eso ponía un
suelo de 4 ms a cada `sleep`, timeout de `poll`/`select`, retransmisión de socket
y despertar programado del sistema.

Ahora `timer_set` reprograma el LAPIC de la CPU llamante para el plazo real. La
dirección es deliberadamente de un solo sentido: **solo adelanta**, nunca
retrasa. Eso importa — el intento anterior (`TICKLESS_IDLE`, que sigue
desactivado) estiraba el periodo para saltarse ticks en una CPU ociosa y dejaba
CPUs detenidas con temporizadores que no vencían nunca, matando la entrada.
Adelantar no puede reproducir ese fallo: en el peor caso una CPU toma más ticks
de los necesarios, y `timer_tick` restablece el límite de 4 ms en cada disparo.

Medido: `nanosleep(1 ms)` retraso medio **2 771 → 991 us** (2,8x mejor), y el
peor caso **5 661 → 1 761 us**, por debajo de los 1 836 us de Linux en el mismo
emulador.

### 4.4 Rodajas de tiempo en nanosegundos, no en ticks

Consecuencia directa de 4.3, y **un fallo introducido por 4.3** que la medición
detectó: `tick_should_preempt` contaba *interrupciones*, así que al variar el
periodo del temporizador la rodaja real de un hilo pasó a depender del tráfico de
temporizadores ajeno. Un vecino con muchos `nanosleep` recortaba la rodaja de
todos, y las preempciones extra costaban más de lo que aportaban:
`pipe round trip` bajo carga empeoró de 2 136 a **16 373 us**.

`SchedAttr` guarda ahora un **plazo absoluto** (`slice_end_ns`) y
`tick_should_preempt` lo compara con el reloj. Con eso, `pipe round trip` bajo
carga cayó a **677 us** — 3,2x mejor que antes de todos los cambios, y 1,8x
mejor que Linux (1 248 us) en el mismo emulador.

### 4.5 Tráfico de cerrojos en la ruta de llamada al sistema

Dos de las cinco tomas de `Thread::inner` por llamada eliminadas: `time` pasa a
`AtomicU64` fuera del mutex, y `put_context` deja de reafirmar un estado que no
cambia (lo que además tomaba el cerrojo de `KObjectBase`).

## 5. Lo que queda, por orden de valor medido

1. ~~**`clock_gettime` entra al núcleo (43x).**~~ **Hecho**: 7 577 ns → 152 ns,
   empate con Linux. Ver 3.quinquies. (Un detalle que resultó ser innecesario:
   musl pide la versión `LINUX_2.6`, pero **omite la comprobación de versión por
   completo** cuando la imagen no tiene `DT_VERDEF` — `if (!verdef) versym = 0;`
   en `src/internal/vdso.c`. No hace falta guion de versiones de símbolos, y
   `build.rs` verifica que no aparezca ninguno, porque introducirlo rompería
   todas las búsquedas en silencio.)

3. **`fork + exec` de un binario grande: 46 ms contra 10,8 ms (4,3x).** Con
   binarios pequeños la diferencia es 1,7x, así que el grueso del coste es por
   byte, no por proceso. La
   causa está localizada: `make_vmo` en `zircon-object/src/util/elf_loader.rs`
   **asigna y copia el segmento entero** en un VMO nuevo en cada `exec` (1,8 MiB
   para busybox), y `LinuxElfLoader::load` mapea y desmapea la imagen completa en
   `KERNEL_ASPACE` alrededor de cada carga (~450 páginas más el shootdown de TLB
   al desmapear). Linux mapea las páginas de la caché de páginas y pagina bajo
   demanda: cero copia. La caché `ELF_VMO_CACHE` ya evita releer el fichero, pero
   no evita ni la copia ni el mapeo.

4. **Eficiencia SMP: 87,6 % contra 98 %**, y **pipe bajo carga 2,6 ms contra
   1,4 ms (1,9x)**. Ambas apuntan al mismo sitio: la colocación de tareas y el
   robo de trabajo del ejecutor solo actúan cuando una CPU se queda *sin nada*
   que hacer; no hay reequilibrado periódico entre CPUs ocupadas de forma
   desigual.

5. **`mprotect`: 183 us contra 101 us (1,8x).** Apunta al shootdown de TLB
   síncrono (`remote_flush_tlb` espera acks con un presupuesto de 32 768 giros).

6. **Rodaja de 20 ms.** Con la preempción por despertar ya no castiga la
   interactividad, pero sigue siendo larga frente a la granularidad efectiva de
   Linux bajo carga (~1-4 ms) para el reparto entre procesos de CPU pura.

7. **`lock_linux()` por llamada al sistema.** `run_user` toma el mutex de
   `LinuxThread` en cada llamada solo para mirar si hay señales pendientes. Un
   espejo atómico del conjunto de señales lo evitaría, pero exige tocar todos los
   puntos que insertan señales; equivocarse ahí es una señal perdida (proceso
   colgado), así que no se ha tocado.

8. **`check_ext_intact` sigue activo dos veces por llamada al sistema.** Su
   propio comentario dice «diagnóstico solamente — quitar cuando se encuentre al
   escritor».

## 6. Contadores del núcleo

`/proc/perf/kernel` publica ahora:

```
timer rearms:   N (R/s, X.XX per tick)
wakeup preempt: N requests (R/s), M honoured (P%)
```

`timer rearms` frente a `timer ticks` dice si la programación por plazo está
comprando precisión (unos pocos rearmes por tick) o ha degenerado en tormenta de
interrupciones (rearmes >> ticks). `wakeup preempt` cuenta los despertares que
cayeron sobre una CPU ocupada con otra tarea y cuántos acortaron efectivamente la
rodaja del hilo en ejecución.
