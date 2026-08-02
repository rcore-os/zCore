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
sistema: es **2-5x más rápido que Linux** en casi todas ellas.

| `[kernel]` | Eclipse | Linux 6.8 | |
| --- | ---: | ---: | --- |
| `getpid()` | 8 705 ns | 26 855 ns | Eclipse **3,1x** |
| `sigprocmask()` | 10 167 ns | 21 685 ns | Eclipse **2,1x** |
| `sched_yield()` | 11 360 ns | 30 831 ns | Eclipse **2,7x** |
| `pread(1 B)` | 12 041 ns | 28 257 ns | Eclipse **2,3x** |
| `write(1 B)` | 9 667 ns | 25 639 ns | Eclipse **2,7x** |
| `fstat()` | 10 042 ns | 33 596 ns | Eclipse **3,3x** |
| `stat("/dev/null")` | 14 186 ns | 66 292 ns | Eclipse **4,7x** |
| `open+close` | 37 956 ns | 129 088 ns | Eclipse **3,4x** |
| `mmap+munmap` | 49 576 ns | 147 651 ns | Eclipse **3,0x** |
| fallo de página menor | 15 992 ns | 89 902 ns | Eclipse **5,6x** |
| pipe ida y vuelta (procesos) | 168 us | 527 us | Eclipse **3,1x** |
| pipe ida y vuelta (hilos) | 70 us | 496 us | Eclipse **7,1x** |
| escalado SMP | 93,7 % | 71,4 % | Eclipse |
| `fork + exit` | 3 348 us | 3 290 us | empate |
| `fork + exec` (estático, 60 KiB) | 9 501 us | 8 699 us | empate |

Y donde Eclipse **sí** pierde — que es exactamente lo que se nota al usarlo:

| `[kernel]` | Eclipse | Linux 6.8 | |
| --- | ---: | ---: | --- |
| `clock_gettime(MONOTONIC)` | 9 232 ns | **187 ns** | Linux **49x** |
| `fork + exec(/bin/sh -c :)` | 42 054 us | 9 246 us | Linux **4,5x** |
| `nanosleep(1 ms)` retraso medio | 2 771 us | 607 us | Linux **4,6x** |
| pipe ida y vuelta bajo carga | 2 136 us | 1 248 us | Linux **1,7x** |
| `mprotect` | 178 781 ns | 96 198 ns | Linux **1,9x** |

Esa es la respuesta a la pregunta original. El sistema no se siente lento porque
las llamadas al sistema lo sean; se siente lento por un puñado de rutas muy
concretas: leer la hora, lanzar un comando, la granularidad de los temporizadores
y la latencia bajo carga.

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

Medido: `nanosleep(1 ms)` retraso medio **2 771 → ~900-1 180 us** (2,3-3x mejor,
consistente entre corridas). Linux en el mismo emulador: 607 us.

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

1. **`clock_gettime` entra al núcleo: 9 232 ns contra 187 ns de Linux (49x).**
   Es, con diferencia, la mayor brecha, y no se cierra optimizando el trap:
   Linux sencillamente **no lo hace**, lo sirve desde la vDSO. Hace falta una
   vDSO real: un DSO ELF mínimo mapeado en cada proceso, con
   `__vdso_clock_gettime` versionado como `LINUX_2.6` (es lo que busca musl en
   `src/internal/vdso.c`), una página de datos compartida con los parámetros del
   TSC, y `AT_SYSINFO_EHDR` en el auxv — que hoy no se emite en absoluto
   (`linux-object/src/loader/abi.rs`). Es la mejora individual más rentable que
   queda y no está hecha.

2. **`fork + exec` de un binario grande: 42 ms contra 9,2 ms (4,5x).** Con
   binarios pequeños hay empate, así que el coste es por byte, no por proceso. La
   causa está localizada: `make_vmo` en `zircon-object/src/util/elf_loader.rs`
   **asigna y copia el segmento entero** en un VMO nuevo en cada `exec` (1,8 MiB
   para busybox), y `LinuxElfLoader::load` mapea y desmapea la imagen completa en
   `KERNEL_ASPACE` alrededor de cada carga (~450 páginas más el shootdown de TLB
   al desmapear). Linux mapea las páginas de la caché de páginas y pagina bajo
   demanda: cero copia. La caché `ELF_VMO_CACHE` ya evita releer el fichero, pero
   no evita ni la copia ni el mapeo.

3. **`mprotect`: 179 us contra 96 us (1,9x).** Apunta al shootdown de TLB
   síncrono (`remote_flush_tlb` espera acks con un presupuesto de 32 768 giros).

4. **Rodaja de 20 ms.** Con la preempción por despertar ya no castiga la
   interactividad, pero sigue siendo larga frente a la granularidad efectiva de
   Linux bajo carga (~1-4 ms) para el reparto entre procesos de CPU pura.

5. **`lock_linux()` por llamada al sistema.** `run_user` toma el mutex de
   `LinuxThread` en cada llamada solo para mirar si hay señales pendientes. Un
   espejo atómico del conjunto de señales lo evitaría, pero exige tocar todos los
   puntos que insertan señales; equivocarse ahí es una señal perdida (proceso
   colgado), así que no se ha tocado.

6. **`check_ext_intact` sigue activo dos veces por llamada al sistema.** Su
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
