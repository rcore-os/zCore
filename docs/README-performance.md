# Por qué Eclipse marcaba como Linux en el benchmark y no lo parecía al usarlo

Este documento explica el desfase entre lo que decía `eclipse-bench` y lo que se
percibía usando el sistema, y qué se ha cambiado en el núcleo para cerrarlo.

## 1. El benchmark no estaba midiendo el sistema operativo

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

Es decir: los titulares del informe eran propiedades del **procesador**, no del
sistema operativo. Dos núcleos distintos sobre la misma máquina *tienen* que dar
los mismos números ahí. Aprobar ese examen no era evidencia de nada.

Y había un segundo sesgo, más importante que el primero: **cada medición corría
sola**. La máquina estaba ociosa salvo por el propio benchmark. Esa es
precisamente la única condición bajo la cual un planificador no puede quedar en
evidencia, porque nunca hay una segunda tarea ejecutable esperando su turno. El
uso real —una shell, un compositor, los demonios y tu programa compitiendo a la
vez— nunca aparecía.

Lo que faltaba por medir, y que domina el tiempo de ejecución de casi cualquier
carga real: latencia de despertar, coste de cambio de contexto, fallos de
página, `mmap`, copy-on-write tras `fork`, resolución de rutas, `clock_gettime`
y escalado SMP.

## 2. La causa real: no había preempción por despertar

Este es el hallazgo principal.

El ejecutor (`vendor/PreemptiveScheduler`) sondea una tarea hasta que su futuro
devuelve `Pending`. Para un hilo de usuario, `run_user` entra en espacio de
usuario y **permanece dentro del mismo sondeo** a través de sucesivas trampas
(llamadas al sistema, fallos de página, ticks de temporizador), regresando al
ejecutor solo cuando expira su rodaja de tiempo:

```rust
// loader/src/linux.rs — antes
if vector == TIMER_INTERRUPT_VEC && thread.tick_should_preempt() {
    kernel_hal::thread::yield_now().await;
}
```

`BASE_TIMESLICE_TICKS = 5` a 250 Hz son **20 ms**. Y despertar una tarea solo
ponía un bit en la página de wakers de su CPU:

```rust
// waker_page.rs — antes
self.page.notify(self.idx);
crate::runtime::maybe_send_resched_ipi(self.page.owner_cpu); // solo si dormía
```

El IPI únicamente se enviaba a una CPU **detenida en `hlt`**. Si la CPU estaba
*ocupada* ejecutando otro hilo, la tarea recién despierta esperaba a que
terminase la rodaja completa del otro: hasta 20 ms. Cada pulsación de tecla,
cada escritura en un pipe, cada finalización de E/S, cada liberación de `futex`
pagaba esa cuenta en cuanto hubiese algo más ejecutable en esa CPU.

Linux resuelve esto con `check_preempt_curr` más un IPI de replanificación: la
tarea que despierta desaloja a la que corre casi de inmediato. Nosotros no
teníamos ese camino.

**Y es invisible para el benchmark antiguo por construcción**: con una sola
tarea ejecutable nunca hay nadie a quien desalojar.

### Corrección

- `NEED_RESCHED`: máscara por CPU con una petición de preempción por despertar
  (`vendor/PreemptiveScheduler/src/runtime.rs`).
- `WakerRef::wake_by_ref` la publica y manda el IPI de replanificación también a
  CPUs **ocupadas**, no solo a las dormidas. Se descartan los despertares de
  tareas ya prestadas a un ejecutor (que es lo que hace `yield_now` consigo
  misma), y la petición se fusiona: una ráfaga de despertares cuesta un IPI, no
  N.
- `handle_user_trap` consume la petición con `take_need_resched()` en **cualquier
  vector de interrupción**, no solo el del temporizador, y cede la CPU. Así el
  IPI se convierte en una cesión inmediata en vez de esperar al siguiente tick.
- El ejecutor limpia el bit al quedarse sin trabajo, para que no suprima el IPI
  del siguiente despertar real.

## 3. El balanceo contaba tareas bloqueadas como carga

`ExecutorRuntime::task_num()` cuenta **todas** las tareas de la colección,
incluidas las aparcadas en un futuro `Pending`. Ese era el número que usaban
tanto la colocación de tareas nuevas como el robo de trabajo.

Consecuencia: una CPU con cincuenta demonios dormidos parecía cincuenta veces
más cargada que una CPU con dos hilos girando a tope. La colocación empujaba
trabajo nuevo *hacia* la CPU saturada, y el robo de trabajo sondeaba primero la
ociosa.

**Corrección**: `TaskCollection::ready_num()` cuenta solo las tareas realmente
ejecutables (`notified & !dropped & !borrowed`), y tanto `spawn_task` como
`steal_task_from_other_cpu` la usan. Se mantiene el `try_lock` en todos los
caminos: una colección momentáneamente bloqueada se omite, nunca se espera.

## 4. Tráfico de cerrojos en la ruta de llamada al sistema

Cada llamada al sistema tomaba el cerrojo `Thread::inner` cinco veces. Dos de
esas veces no hacían falta:

- `time_add` guardaba el tiempo de usuario **dentro** del mutex, en cada regreso
  de espacio de usuario, solo para sumar un número que nada más lee bajo ese
  cerrojo. Ahora es un `AtomicU64` fuera de `inner`.
- `put_context` reafirmaba el estado del hilo llamando a `change_state` con el
  estado que ya tenía, lo que además tomaba el cerrojo de `KObjectBase`. Para un
  hilo simplemente en ejecución es una operación nula; ahora se omite (se
  conserva íntegra cuando hay un `zx_task_suspend` pendiente, que es el único
  caso en que sí cambia algo).

## 5. Lo que queda pendiente

Anotado aquí por honestidad, no implementado:

- **`clock_gettime` entra al núcleo.** Linux lo sirve desde la vDSO sin trampa
  (~25 ns). Es de las operaciones más frecuentes que emite cualquier programa
  real. Una vDSO es la mejora individual más rentable que queda.
- **`lock_linux()` por llamada al sistema.** `run_user` toma el mutex de
  `LinuxThread` en cada llamada solo para comprobar si hay señales pendientes.
  Un espejo atómico del conjunto de señales lo evitaría, pero exige tocar todos
  los puntos que insertan señales; hacerlo mal significa una señal perdida (un
  proceso colgado), así que no se ha tocado sin poder ejecutar la suite completa.
- **`check_ext_intact` sigue activo en cada llamada al sistema**, dos veces. Su
  propio comentario dice «diagnóstico solamente — quitar cuando se encuentre al
  escritor». Es barato, pero es peso muerto en la ruta más caliente del núcleo.
- **Rodaja de 20 ms.** Con la preempción por despertar ya no castiga la
  interactividad, pero sigue siendo larga frente a la granularidad efectiva de
  Linux bajo carga (~1–4 ms) para el reparto entre procesos puramente de CPU.
  Es un ajuste con contrapartidas; conviene medirlo antes de tocarlo.

## 6. Cómo verificarlo

```sh
# En Eclipse
./eclipse-bench --only sched .

# La misma binaria en Linux, en la misma máquina
./eclipse-bench --only sched .
```

El número a mirar es `wake late loaded/idle (worst)` en el bloque `RATIOS`:
cuánto se degrada la latencia de despertar al saturar todas las CPUs. Cerca de 1
significa que una tarea que despierta consigue CPU enseguida aunque la máquina
esté ocupada. Un valor alto significa que espera a que se agote la rodaja de
otro, y el sistema se sentirá lento por muy buenos que sean los números `[user]`.

Del lado del núcleo, `/proc/perf/kernel` publica ahora:

```
wakeup preempt: N requests (R/s), M honoured (P%)
```

`requests` son los despertares que cayeron sobre una CPU ocupada con otra tarea;
`honoured` son los que efectivamente acortaron la rodaja del hilo en ejecución.
Un déficit grande significa que las peticiones aterrizan en CPUs que permanecen
en modo núcleo, donde la ruta de trampas no llega a verlas.
