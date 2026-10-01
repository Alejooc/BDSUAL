# Spec 007 — Editor y ejecución SQL

Estado: lectura MySQL/MariaDB/PostgreSQL/SQLite implementada parcialmente; ejecución SQL modificadora general pendiente
Versión: 0.7
Última actualización: 2026-09-30
Depende de: [001 — Conexiones](001-conexiones.md), [005 — Interfaz](005-interfaz-de-trabajo.md), [006 — Control de cambios](006-control-de-cambios.md)

## Objetivo

Permitir escribir SQL en pestañas ligadas a una conexión y una base de datos, ejecutar consultas y revisar sus resultados. MySQL se implementará primero; MariaDB, PostgreSQL y SQLite seguirán después con sus dialectos y capacidades.

## Editor

- Cada pestaña muestra motor, conexión y base de datos de destino. Cambiar la selección del explorador no cambia el destino de la pestaña.
- Monaco Editor ofrece resaltado de sintaxis SQL, números de línea y diagnóstico visible cuando una ejecución falla.
- Se puede ejecutar la selección actual o, si no hay selección, el contenido completo del editor. Antes de enviar varias instrucciones, la interfaz muestra que se trata de una ejecución múltiple.
- El contenido no guardado se indica en la pestaña y se conserva o se solicita una decisión explícita al cerrarla.
- El texto del editor puede contener datos sensibles. No se copia automáticamente al historial de conexiones, registros de error ni telemetría.

## Ejecución de consultas de lectura

1. La persona usa **Ejecutar lectura** para consultas cuyo propósito es inspeccionar datos o metadatos.
2. El core aplica un modo de solo lectura cuando el motor y la operación lo permitan. Si no puede garantizarlo, no presenta la acción como segura y ofrece preparar el SQL como cambio.
3. El panel de resultados muestra columnas, filas recibidas, tiempo de ejecución y si quedan resultados por cargar.
4. Se puede cancelar una consulta en curso mediante `KILL QUERY` desde otra sesión del mismo pool. La interfaz confirma la cancelación solo cuando el servidor interrumpe la lectura; ante error o cierre informa que no pudo verificarla.
5. Una consulta con demasiados resultados no bloquea la interfaz ni se trunca sin aviso. En MySQL se pueden cargar más filas por tramos con `LIMIT/OFFSET` acotado. Cada tramo vuelve a ejecutar la consulta; datos concurrentes pueden desplazar o repetir filas, por lo que se recomienda ordenar por una clave estable.
6. El presupuesto nominal de 1 MB por respuesta reduce el número de filas de cada tramo en fronteras de fila; no rechaza la consulta ni trunca celdas. Si una fila supera por sí sola el presupuesto, se entrega completa y la siguiente página continúa después de esa fila.

## SQL que modifica la base

- Las instrucciones que cambian datos o estructuras se envían siempre a **Cambios preparados**. Confirmar la revisión no ejecuta SQL; **Aplicar a la base** usa el flujo del spec 006 y las reglas de conflictos, soporte y resultado por instrucción del [spec 015](015-conflictos-y-aplicacion.md).
- Las instrucciones cuyo efecto no se pueda determinar de manera fiable también se preparan como cambio, con una advertencia de vista previa incompleta.
- La vista previa muestra el texto SQL, el destino y los efectos que se pudieron identificar. Nunca promete que el análisis de SQL arbitrario haya encontrado todos los efectos posibles.
- Si el historial local no está disponible, la aplicación no ofrece una vía de ejecución directa para SQL que pueda modificar la base.
- Los resultados de cada instrucción se asocian a su ejecución: éxito, filas afectadas cuando el motor lo informa, error y estado incierto si se perdió la conexión.

### Implementación parcial actual

- El editor tiene una acción separada **Preparar cambio**. Acepta una sola instrucción no lectora de hasta 64 KiB y la guarda como artefacto cifrado local asociado a la conexión y la base verificadas.
- Las lecturas usan IDs efímeros de consulta; **Cancelar** envía `KILL QUERY` por otra sesión del mismo pool. El ID de sesión se obtiene del propio socket de lectura, y el registro activo se quita al terminar la operación.
- Al desconectar, el core retira primero el pool de la lista activa, cancela las consultas registradas de esa conexión y después cierra el pool. La interfaz invalida respuestas pendientes al comenzar el cierre para no mostrar filas tardías. Hay una prueba unitaria que comprueba que el filtro de cancelación solo incluye consultas de la conexión seleccionada; falta la integración real de desconexión concurrente contra MySQL.
- La sección **Cambios preparados** puede volver a abrir el plan autenticando el artefacto, mostrar su texto, estado y hash, y confirmar la revisión local. No ejecuta el SQL.
- La aplicación de SQL modificador general queda **Pendiente**. Hay creación de base MySQL vacía y cambios localizados de filas fuera del editor según sus propios specs; el editor conserva su gate hasta disponer de un proveedor de recuperación adecuado para cada efecto. El proveedor actual restaura solo cobertura parcial MySQL/MariaDB a una base nueva y no sirve como respaldo completo para SQL arbitrario, que puede afectar rutinas, vistas, eventos y otras dependencias.
- La lectura y validación se limita al dialecto MySQL/MariaDB. Una integración desechable comprobó en MariaDB 10.6.28, 10.11.19 y 11.4.13 que se puede ejecutar una lectura acotada junto con la conexión y la exploración básica. Los lotes de 200 filas, la lectura de filas vacías y la cancelación confirmada se comprobaron contra MySQL 8.4.11. La cancelación automática al desconectar ya está cableada, pero aún falta verificarla con una consulta larga en servidores reales. La estabilidad de páginas ante escrituras concurrentes también sigue pendiente. La administración del historial/restauración sigue deshabilitada para MariaDB.
- PostgreSQL admite una sentencia `SELECT` por ejecución mediante un comando propio. Cada lectura abre el contexto de la base seleccionada, usa una transacción `READ ONLY`, aplica `statement_timeout` de 15 segundos y limita la respuesta a 200 filas y cerca de 1 MiB por página. Se rechazan lotes, instrucciones distintas de `SELECT` y `SELECT INTO`; columnas duplicadas exigen alias únicos. Una integración PostgreSQL 16 verificó `SELECT`, valores `NULL`, páginas de 200 filas y que `nextval` no avance una secuencia. Playwright verifica la selección del motor y el recorrido IPC simulado. Cancelación PostgreSQL y preparación/aplicación de cambios PostgreSQL siguen pendientes; no hay recorrido nativo verificado.

## Errores y contexto

- Si no hay conexión abierta o base de datos de destino, la ejecución se desactiva y se indica qué falta.
- Los errores del motor se muestran sin credenciales ni cadenas de conexión con secretos. Cuando se conoce la posición, el editor marca la instrucción y ubicación afectadas.
- Al desconectar durante una consulta, se detiene la carga de resultados y no se muestran filas tardías como resultado vigente.
- Una pestaña no ejecuta SQL contra otro destino tras reconectar sin que la persona revise el contexto visible.
- La aplicación no registra como exitosas instrucciones cuya confirmación del motor no recibió.

## Criterios de aceptación iniciales: MySQL

1. Dada una pestaña SQL ligada a una base MySQL, ejecutar una consulta de lectura muestra columnas, filas y tiempo sin cambiar el destino al seleccionar otra base en el explorador.
2. Dada una consulta de lectura con más filas que el primer lote, la interfaz indica que hay más resultados y permite cargar tramos siguientes; también advierte que cambios concurrentes pueden desplazar filas.
3. Dado SQL que modifica datos, la acción crea un cambio preparado y la base permanece intacta hasta **Aplicar a la base**.
4. Dado SQL cuyo efecto no puede clasificarse con certeza, la interfaz lo trata como posible cambio y no lo ejecuta bajo la acción de lectura.
5. Dado un error de sintaxis con posición disponible, el mensaje y la marca del editor apuntan a la instrucción afectada.
6. Dada una desconexión durante la ejecución, el resultado se marca fallido o incierto según la evidencia; no se anuncia éxito sin confirmación del servidor.
7. Cerrar una pestaña con SQL sin guardar conserva el texto o pide una decisión antes de descartarlo.

## Criterios iniciales PostgreSQL implementados parcialmente

1. Una conexión abierta puede ejecutar una sentencia `SELECT` en la base elegida y mostrar nombres de columnas, filas, `NULL` y duración.
2. La base impone una transacción `READ ONLY`; intentos de escritura como `nextval` fallan sin dejar cambios, y el servidor aplica un tiempo máximo de 15 segundos.
3. Cada respuesta contiene como máximo 200 filas y permite solicitar páginas posteriores; un alias repetido en el resultado produce un error explícito.
4. La interfaz usa el comando PostgreSQL dedicado. Preparar cambios y cancelar lecturas se mantienen deshabilitados para PostgreSQL hasta implementar sus contratos propios.
5. La prueba de aceptación de producto debe recorrer una ventana Tauri nativa contra PostgreSQL real; preview y Playwright simulado no la reemplazan.

## Fuera de este spec

- Autocompletado avanzado basado en todo el esquema.
- Formateo automático de SQL.
- Depurador de procedimientos almacenados.
- Comparación visual de planes de ejecución.
- Programación periódica de consultas.

## Decisiones pendientes

1. Cómo guardar consultas nombradas y si se sincronizan con un proyecto.
2. El lote inicial de 200 filas y el desplazamiento máximo de 1.000.000 están fijados por el core; se podrán volver configurables al perfilar consultas grandes.
3. Matriz de instrucciones múltiples admitidas por versión de cada motor; el tratamiento general de commits implícitos se define en el spec 015.

## Consultas SQLite en el editor — 2026-09-30

El editor SQL ya acepta conexiones SQLite y envía lecturas `SELECT` al comando Tauri. El archivo permanece abierto en modo de solo lectura; el core restringe la consulta a una sentencia de lectura, aplica 15 segundos de timeout y pagina resultados con máximo 200 filas y 1 MiB nominal. Preparar cambios permanece deshabilitado. La exportación SQL y las escrituras SQLite siguen fuera del alcance hasta completar historial y respaldo protegido.
