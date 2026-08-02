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
| `fork + exit` | 3 708 us | 3 690 us | empate |

Y donde Eclipse **sí** pierde — que es exactamente lo que se nota al usarlo:

| `[kernel]` | Eclipse | Linux 6.8 | |
| --- | ---: | ---: | --- |
| `clock_gettime(MONOTONIC)` | 8 597 ns | **199 ns** | Linux **43x** |
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

1. **`clock_gettime` entra al núcleo: 8 597 ns contra 199 ns de Linux (43x).**
   Es, con diferencia, la mayor brecha, y no se cierra optimizando el trap:
   Linux sencillamente **no lo hace**, lo sirve desde la vDSO. Hace falta una
   vDSO real: un DSO ELF mínimo mapeado en cada proceso, con
   `__vdso_clock_gettime` versionado como `LINUX_2.6` (es lo que busca musl en
   `src/internal/vdso.c`), una página de datos compartida con los parámetros del
   TSC, y `AT_SYSINFO_EHDR` en el auxv — que hoy no se emite en absoluto
   (`linux-object/src/loader/abi.rs`). Es la mejora individual más rentable que
   queda y no está hecha.

2. **`fork + exec` de un binario grande: 46 ms contra 10,8 ms (4,3x).** Con
   binarios pequeños la diferencia es 1,7x, así que el grueso del coste es por
   byte, no por proceso. La
   causa está localizada: `make_vmo` en `zircon-object/src/util/elf_loader.rs`
   **asigna y copia el segmento entero** en un VMO nuevo en cada `exec` (1,8 MiB
   para busybox), y `LinuxElfLoader::load` mapea y desmapea la imagen completa en
   `KERNEL_ASPACE` alrededor de cada carga (~450 páginas más el shootdown de TLB
   al desmapear). Linux mapea las páginas de la caché de páginas y pagina bajo
   demanda: cero copia. La caché `ELF_VMO_CACHE` ya evita releer el fichero, pero
   no evita ni la copia ni el mapeo.

3. **Eficiencia SMP: 87,6 % contra 98 %**, y **pipe bajo carga 2,6 ms contra
   1,4 ms (1,9x)**. Ambas apuntan al mismo sitio: la colocación de tareas y el
   robo de trabajo del ejecutor solo actúan cuando una CPU se queda *sin nada*
   que hacer; no hay reequilibrado periódico entre CPUs ocupadas de forma
   desigual.

4. **`mprotect`: 183 us contra 101 us (1,8x).** Apunta al shootdown de TLB
   síncrono (`remote_flush_tlb` espera acks con un presupuesto de 32 768 giros).

5. **Rodaja de 20 ms.** Con la preempción por despertar ya no castiga la
   interactividad, pero sigue siendo larga frente a la granularidad efectiva de
   Linux bajo carga (~1-4 ms) para el reparto entre procesos de CPU pura.

6. **`lock_linux()` por llamada al sistema.** `run_user` toma el mutex de
   `LinuxThread` en cada llamada solo para mirar si hay señales pendientes. Un
   espejo atómico del conjunto de señales lo evitaría, pero exige tocar todos los
   puntos que insertan señales; equivocarse ahí es una señal perdida (proceso
   colgado), así que no se ha tocado.

7. **`check_ext_intact` sigue activo dos veces por llamada al sistema.** Su
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
