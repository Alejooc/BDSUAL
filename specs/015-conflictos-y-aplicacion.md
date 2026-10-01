# Spec 015 — Conflictos y aplicación de revisiones

Estado: borrador para revisión  
Versión: 0.1  
Última actualización: 2026-09-28  
Depende de: [006 — Control de cambios](006-control-de-cambios.md), [007 — Editor SQL](007-editor-sql.md), [008 — Cuadrícula](008-cuadricula-datos.md), [009 — Arquitectura](009-arquitectura.md), [010 — Respaldos](010-respaldos-y-recuperacion.md)

## Objetivo

Impedir que una revisión confirmada sobrescriba silenciosamente cambios externos y describir el resultado real de una aplicación que falle, se interrumpa o solo se complete en parte. El flujo obligatorio sigue siendo **Preparar → Revisar → Confirmar revisión → Aplicar a la base**.

## Evidencia guardada al confirmar

La revisión fija el identificador del proyecto y del destino, motor, versión conocida, operaciones ordenadas, hash del SQL o archivo de entrada, esquema y objetos afectados, filas identificables y valores originales necesarios para detectar conflictos. No se confía solo en lo que mostraba la interfaz: el core guarda y vuelve a consultar las precondiciones.

La vista previa indica el **nivel de certeza** de cada operación:

| Nivel | Qué se conoce | Tratamiento |
| --- | --- | --- |
| Fila identificable | Clave estable y valores originales comparables. | Comparar al aplicar; compensación posible si no hay efectos indirectos. |
| Objeto conocido | Identidad y definición de tabla, vista u otro objeto. | Comparar el estado del catálogo y exigir respaldo cuando corresponda. |
| Alcance incierto | SQL, rutinas, disparadores o dependencias que el adaptador no puede acotar. | Mostrar límites; exigir respaldo completo y compatibilidad explícita del adaptador. |

Si un plan mezcla niveles, se aplica el requisito más exigente de toda la revisión. Un respaldo completo permite recuperar un estado anterior; no demuestra que el SQL arbitrario sea inocuo ni que pueda aplicarse sin conflictos.

## Comprobaciones antes de aplicar

1. El core adquiere el bloqueo local del proyecto, verifica que la revisión sigue confirmada y que sus artefactos coinciden con los hashes guardados.
2. Abre de nuevo el destino y comprueba conexión, base, permisos y objetos implicados. Si un identificador o definición relevante cambió, deja la revisión **Confirmada sin aplicar**, registra el motivo **Conflicto antes de aplicar** y no envía ninguna operación.
3. Compara las filas editadas o eliminadas mediante su clave y sus valores originales o una versión fiable de fila. Una fila ausente, duplicada o modificada es un conflicto; nunca se usa la posición visible en la cuadrícula.
4. Prepara y verifica la protección del spec 010. Una falla deja la revisión confirmada sin aplicar.
5. Inmediatamente antes de cada escritura, vuelve a comprobar sus precondiciones dentro de la misma transacción o bloqueo del motor cuando sea posible. Si el motor no permite cerrar la ventana entre comprobación y escritura, se informa el límite y se verifica el resultado después.

### Gate PostgreSQL

PostgreSQL no tiene todavía proveedor de respaldo completo validado (spec 012), por lo que la aplicación de filas PostgreSQL debe permanecer deshabilitada. No se debe conectar una función `UPDATE` al historial hasta que el artefacto de recuperación completo se genere, autentique y restaure contra versiones admitidas, y hasta que se haya probado en PostgreSQL real la comparación concurrente, resultado verificado y transición a `uncertain` sin reintento automático. Una integración que solo compruebe sintaxis, lectura de metadatos o SQL generado no satisface esta puerta.

DBSUAL no combina automáticamente el cambio externo con el borrador ni cambia el SQL confirmado. La persona puede inspeccionar el estado nuevo y crear una revisión actualizada; la revisión original permanece en el historial.

## Evidencia IPC en Windows

El test de integración `tauri_mock_ipc` construye un `MockRuntime` de Tauri, registra el comando de producción `confirm_history_revision` y envía una invocación IPC con `revisionId` inválido. La respuesta serializada conserva el código estable `INVALID_HISTORY_REVISION`. El target usa la feature de prueba de Tauri solo como dependencia de desarrollo y manifiesto Common Controls v6. Esta comprobación cubre despacho y validación de entrada; la preparación, aplicación, detección de conflicto y compensación MySQL a través de IPC con servidor desechable siguen pendientes.

## Aplicación y resultado por paso

- El core registra de forma durable que el trabajo comenzó y qué paso va a enviar. Por cada paso conserva **pendiente**, **confirmado**, **fallido** o **incierto**, junto con la evidencia de la respuesta del motor.
- Cuando todos los pasos admiten una misma transacción y el adaptador lo verifica, se ejecutan juntos. Ante error se solicita reversión y se comprueba su resultado. Solo se informa «no se aplicó» si esa reversión quedó confirmada.
- Si un paso causa commit implícito o debe ejecutarse fuera de una transacción, los pasos ya confirmados permanecen como tales. El proveedor se detiene en el primer fallo y no inicia los siguientes. La revisión termina **Fallido parcialmente** si conoce el estado de cada paso ejecutado.
- Si se pierde la conexión, se cierra la app o falta confirmación del motor, el paso enviado queda **incierto**. DBSUAL no lo reintenta automáticamente ni presume que el servidor lo revirtió. Al volver, consulta el destino para reconciliar; si sigue sin poder determinar el efecto, mantiene **Estado incierto** y bloquea una nueva aplicación sobre ese proyecto hasta que se revise.
- Un reintento explícito de una operación potencialmente no idempotente requiere nueva revisión y nueva comprobación del destino. El historial no duplica una inserción solo porque faltó su respuesta.

## Reglas por origen de cambio

### Cuadrícula y CSV

- Actualizar o borrar una fila exige que la condición de escritura identifique exactamente la fila prevista y detecte cambios relevantes. Una coincidencia de cero o de más de una fila provoca conflicto; la transacción se revierte cuando el motor lo permite.
- Insertar filas verifica restricciones y registra las claves efectivamente generadas. Si no se recibió confirmación de una inserción, no se envía de nuevo a ciegas.
- CSV usa por defecto **todo o nada por tabla** cuando el motor y la tabla permiten una transacción real. En una tabla no transaccional, la vista previa declara que puede haber filas aplicadas parcialmente; requiere respaldo completo y un proveedor capaz de informar las filas confirmadas. Si no puede hacerlo, la importación queda bloqueada.
- Estado de implementación: el parser común solo valida entrada CSV UTF-8 acotada y no autoriza escrituras. Hasta completar y probar precondiciones del destino, artefacto cifrado, recuperación y transacción, no hay importación CSV disponible.

### SQL e importación SQL

- El analizador identifica instrucciones y orden cuando puede. Instrucciones de control de transacciones, cambio de conexión o destino, acceso a archivos externos y funciones con efectos no acotables requieren soporte explícito del adaptador; si no existe, se rechazan **antes de aplicar** con el motivo visible.
- Una instrucción de efecto desconocido que sí admita el adaptador exige respaldo completo, vista previa marcada como incompleta y verificación posterior. No se anuncia ejecución atómica si hay DDL u otros pasos que hacen commit implícito.
- Una importación SQL generada por DBSUAL se prueba en una base aislada del mismo motor. Un archivo externo se valida y se prepara como revisión, pero no se promete admitir cualquier instrucción del dialecto.
- No se reejecuta automáticamente un archivo completo tras un error parcial. La interfaz muestra pasos confirmados, fallidos, pendientes e inciertos, y permite preparar una nueva revisión sobre el estado observado.

## Presentación de conflictos

La vista de revisión muestra **valor esperado**, **valor actual** y **valor propuesto** para filas cuando es seguro revelarlos, y muestra la definición esperada y actual para objetos. Una diferencia que contiene datos sensibles se protege como artefacto del historial y no se registra en texto claro. Las acciones disponibles son volver a inspeccionar, editar el borrador y confirmar una nueva revisión, o abrir el punto de recuperación existente; ninguna fuerza silenciosamente el cambio anterior.

## Criterios de aceptación

1. Una fila cambiada por otra sesión entre confirmación y aplicación no se sobrescribe; el historial registra el conflicto antes de aplicar.
2. Si otra sesión cambia una fila durante la aplicación, la condición de escritura detecta la diferencia y no actualiza otra fila ni una versión inesperada.
3. Una revisión con varios pasos transaccionales falla a mitad y confirma la reversión sin declarar pasos parciales cuando el motor realmente los revirtió.
4. Una revisión MySQL o MariaDB que combina DDL y otros pasos muestra sus límites antes de aplicar; si falla tras un commit implícito, conserva los pasos confirmados y no anuncia rollback completo.
5. Tras perder la conexión inmediatamente después de enviar un `INSERT`, DBSUAL no lo reenvía automáticamente; reconcilia o conserva **Estado incierto**.
6. Una importación CSV transaccional con una fila inválida no deja filas nuevas; en una tabla no transaccional se informa el riesgo antes de aplicar o se bloquea si no puede medirse el resultado.
7. Cambiar el archivo SQL después de confirmar la revisión impide aplicarla hasta preparar y confirmar otra revisión.
8. Una instrucción externa no admitida se rechaza antes de enviar ningún paso de la revisión al motor.
9. PostgreSQL sin proveedor de respaldo probado no expone ni acepta acciones de escritura desde la cuadrícula o el editor, aunque la conexión y lectura estén disponibles.

## Decisiones pendientes

1. Matriz exacta de instrucciones SQL e importaciones admitidas por versión de cada motor.
2. Método de comparación de valores grandes o tipos especiales y reglas de concurrencia para tablas sin columna de versión.
3. Bloqueos y tiempos máximos por motor para reducir conflictos sin detener trabajo ajeno durante demasiado tiempo.

## Referencias técnicas

- [MySQL: instrucciones con commit implícito](https://dev.mysql.com/doc/refman/8.4/en/implicit-commit.html).
- [MariaDB: instrucciones con commit implícito](https://mariadb.com/docs/server/reference/sql-statements/transactions/sql-statements-that-cause-an-implicit-commit).
- [PostgreSQL: `DROP DATABASE` fuera de transacciones](https://www.postgresql.org/docs/18/sql-dropdatabase.html).
- [SQLite: transacciones y respuesta ante errores](https://www.sqlite.org/lang_transaction.html).
