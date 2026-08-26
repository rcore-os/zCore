# Contención de fallos del kernel (oops)

Un defecto en un programa de usuario no debe llevarse por delante el sistema
operativo. Este documento describe qué parte de eso ya estaba resuelta, qué
faltaba, y el mecanismo que cubre lo que faltaba.

## Lo que ya funcionaba: fallos *del programa*

Cuando el fallo lo comete el propio programa —desreferenciar un puntero nulo,
dividir por cero, ejecutar una instrucción ilegal, saltar a través de un puntero
corrupto— la CPU lanza una excepción en modo usuario y `handle_user_trap`
(`loader/src/linux.rs`) la traduce a la señal Linux equivalente:

| Trampa | Señal |
|---|---|
| fallo de página no resoluble contra el VMAR | `SIGSEGV` |
| instrucción indefinida (`#UD`) | `SIGILL` |
| división por cero (`#DE`), FPU (`#MF`/`#XF`) | `SIGFPE` |
| acceso no alineado, segmento no presente (`#NP`) | `SIGBUS` |
| protección general (`#GP`), pila (`#SS`) | `SIGSEGV` |
| punto de ruptura | `SIGTRAP` |

El proceso muere (con su volcado de registros, bytes de código alrededor del PC
y el mapeo al que pertenecía la dirección culpable, todo en `dmesg`) y el
sistema sigue. Eso no ha cambiado.

## Lo que faltaba: fallos *del kernel atendiendo a ese programa*

El caso que sí paraba la máquina era el contrario: una llamada al sistema hecha
por el programa que hacía fallar al núcleo. Un `panic!`, un `unwrap` sobre un
argumento que nadie esperaba, un fallo de página en una ruta de driver — todo
eso terminaba en `lang.rs` pintando su banner rojo y entrando en
`loop { spin_loop() }`. Todos los demás procesos morían con él, incluida la
shell desde la que se podría haber diagnosticado.

Es la misma distinción que hace Linux entre un *oops* (fallo del kernel en
contexto de proceso: se mata al proceso y el núcleo sigue, marcado como
«tainted») y un *panic* (parada total). Hasta ahora Eclipse sólo tenía lo
segundo.

## El mecanismo

`zCore/src/oops.rs`. Cuando la ruta de fallo termina de imprimir su
diagnóstico, llama a `oops::try_contain`, que si puede:

1. **Mata al proceso culpable** por el mismo camino que un `kill -9`
   (`Process::exit(128 + SIGKILL)`), de modo que el `wait4` del padre y el `$?`
   de la shell lo vean como lo que es.
2. **Retira la corrutina** que estaba fallando: se marca como terminada, se
   saca de la colección de tareas y su `Future` se sustituye por uno inerte.
   El original se *fuga* a propósito — véase «Lo que cuesta».
3. **Devuelve la CPU al planificador** cambiando de contexto a la pila del
   runtime. El ejecutor abandonado queda marcado como muerto, así que el
   runtime crea uno nuevo en su lugar y nunca vuelve a esa pila.

Si no puede, imprime *por qué* y se para como antes.

## Cuándo NO se contiene

La recuperación sólo es segura si se puede salir del fallo sin dejar el núcleo a
medias. Se rechaza —y la máquina se para, con el motivo en la consola— cuando:

- **La CPU tiene algún cerrojo del kernel cogido** (`lock::lock_depth() != 0`).
  Todo `MutexGuard` de `kernel-sync` enmarca su vida con `push_off`/`pop_off`,
  así que `noff == 0` demuestra que no hay ninguno vivo: nada quedó a medio
  modificar y ningún cerrojo se quedará cogido para siempre.
- **El fallo llegó en contexto de interrupción** (`in_timer_callback`). No hay
  proceso al que atribuirlo: el hilo interrumpido es una coincidencia.
- **No había ninguna corrutina ejecutándose**, la pila que falló no es la suya
  (p. ej. un `#GP`, que tiene pila IST propia), o su canario de desbordamiento
  ya está pisado.
- **Ya se sospechaba corrupción del montón** (`heap_smash_suspected`). Con el
  montón pisado, seguir ejecutando sólo propaga el daño.
- **El fallo es un golpe contra la guarda de pila** (`stack_guard`). Ese caso ni
  siquiera llega a `try_contain`: `handle_page_fault` informa y para. La sonda
  que falló era la página *siguiente* hacia abajo, así que lo que queda de pila
  bajo `RSP` es sólo lo que este marco aún no había reclamado; ejecutar ahí la
  contención (formatear, `Process::exit`) volvería a crecer contra la guarda, y
  esta vez con `RSP` ya dentro de ella la CPU no puede ni apilar el marco de
  excepción: doble fallo. Contener un desbordamiento requiere primero que el
  manejador de `#PF` corra en pila propia (IST), como ya hacen `#DF` y `#GP`.
- **Ya se estaba conteniendo otro fallo en esa misma CPU**: si la maquinaria de
  recuperación es lo que falla, no hay nada que rescatar.
- **Se agotó el presupuesto** (`MAX_CONTAINED = 16` fallos por arranque).

Estas condiciones son necesarias, no suficientes. Quedan fuera de la
comprobación unos pocos `spin::Mutex` externos (planificador, consola,
`init_once`) que no cuentan en `lock_depth`. El caso que importa —los objetos
del núcleo: proceso, hilo, VMAR, ficheros— sí usa `lock::Mutex` y sí está
cubierto.

## Lo que cuesta

Abandonar una corrutina a media ejecución **no deshace lo que estaba haciendo**:
no se ejecuta ningún destructor de la cadena de llamadas interrumpida. Se
pierden su memoria, sus referencias `Arc` (incluidos los objetos `Process` y
`Thread` de la víctima, aunque no sus mapeos: `Process::terminate` libera el
espacio de direcciones) y su pila de corrutina de 2 MiB. Es fuga acotada por
fallo contenido, y de ahí el presupuesto: un núcleo que se pasa el día
conteniendo fallos no está sano, y llegado ese punto es preferible una parada
que se pueda diagnosticar.

Tampoco se re-ejecuta ni se destruye el `Future` interrumpido. Su generador
quedó con el discriminante del último punto de `await`, que ya no describe los
valores que realmente contiene: reanudarlo repetiría el código que falló y
destruirlo podría liberar dos veces algo que el `poll` abortado ya había
consumido. Por eso se sustituye por un `Pending` inerte y el original se fuga.

## Cómo se ve

El diagnóstico completo de siempre (banner de pánico, retroceso de pila, volcado
del fallo de página) se imprime **antes** de contener, así que no se pierde
nada. Detrás aparece una de estas dos líneas:

```
[oops] kernel panic contained (1/16): killing pid=1234 "firefox" tid=1235 —
       the kernel stays up, no other process is affected
```

```
[oops] kernel panic NOT contained (the CPU holds 1 kernel lock(s)) — halting
```

Un pánico posterior indica además cuántos fallos lleva contenidos el arranque,
porque lo habitual es que sea la misma causa raíz volviendo:

```
[oops] kernel faults already contained this boot: 3
```

Si la corrutina era del propio kernel (sondeo de red, trabajo diferido de un
driver) no hay proceso al que culpar; se retira igualmente —parar la máquina es
estrictamente peor— pero se avisa de que ese subsistema puede quedar inoperativo
hasta el próximo arranque.

## Desactivarlo

```ini
cmdline=LOG=warn:PANICONOOPS=1
```

Devuelve el comportamiento anterior: parada en el primer fallo. Es lo que se
quiere mientras se depura un fallo concreto, porque deja la máquina congelada en
el sitio, con la corrutina culpable sin desmontar y su pila intacta para
inspeccionarla desde el monitor de QEMU. Por defecto la contención está
**activa**.

## Diseño: por qué un cambio de contexto y no un `longjmp`

Salir de un fallo exige abandonar la pila donde ocurrió. La alternativa clásica
—un `setjmp`/`longjmp` a un punto de recuperación— no encaja aquí: las variables
locales de una `async fn` viven en el objeto `Future` (el montón), no en la pila
de la máquina, así que un `jmp_buf` capturado antes de un `.await` deja de ser
válido en cuanto la corrutina se suspende y se reanuda.

El planificador, en cambio, ya sabe cambiar de pila: es lo que hace en cada
expropiación por temporizador (`sched_yield` degrada el ejecutor actual y crea
otro). La contención reutiliza exactamente ese camino, sólo que marcando el
ejecutor como muerto para que nadie lo reanude jamás. El código nuevo del
planificador se reduce a `runtime::abandon_current_task`, que es `sched_yield`
más «y no vuelvas».
