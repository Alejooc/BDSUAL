# Spec 011 — Proveedor de respaldos MySQL y MariaDB

Estado: captura y restauración parcial MySQL y MariaDB (tablas, vistas locales y triggers simples); no cumple recuperación completa
Versión: 0.6
Última actualización: 2026-09-30
Depende de: [004 — Importación y exportación](004-importacion-exportacion.md), [010 — Respaldos y recuperación](010-respaldos-y-recuperacion.md)

## Objetivo

Definir el primer proveedor de copia completa y restauración para bases MySQL y MariaDB en Windows, macOS y Linux. Ambos servidores comparten parte del acceso a consultas, pero se identifican y prueban por separado para respaldos y restauraciones; la distribución de herramientas nativas se valida por target conforme al spec 019.

## Detección y compatibilidad

- Al abrir la conexión, el core identifica familia y versión del servidor. La familia detectada debe coincidir con la configuración antes de elegir un proveedor de respaldo.
- El proveedor registra versión de servidor, versión de la herramienta y formato de la copia. Una copia de MySQL no se ofrece como restaurable en MariaDB, ni a la inversa, sin una prueba específica de compatibilidad.
- Matriz de versiones para aceptación del MVP: MySQL 8.0 y 8.4; MariaDB 10.6, 10.11 y 11.4. Cada versión se prueba en copia y restauración, además de las operaciones básicas del adaptador.
- Si una versión no está en la matriz validada, la aplicación puede permitir consultas y exploración, pero no promete respaldo protegido ni habilita operaciones destructivas que dependan de él.

## Alcance de una copia «completa»

La copia protegida de una base incluye, cuando existan y sean accesibles, definición de la base, tablas y datos, vistas, índices, claves y restricciones, disparadores, procedimientos, funciones y eventos. Conserva opciones relevantes de codificación e intercalación para restaurarlas en una versión compatible.

Los usuarios, roles y ajustes globales de la instancia quedan fuera de una copia de una sola base. La interfaz lo indica antes de crear o restaurar. Si falta permiso para leer una categoría incluida, la copia falla como respaldo protegido; no se acepta silenciosamente una copia parcial.

## Estrategia de herramientas

- Se evalúa `mysqldump` para MySQL y `mariadb-dump` para MariaDB, con sus respectivos clientes de restauración. La implementación debe fijar opciones explícitas para incluir rutinas, eventos y disparadores, no depender solo de los valores predeterminados.
- La aplicación debe poder ejecutar el proveedor en cada sistema operativo declarado compatible sin exigir una instalación manual no documentada. Antes de distribuirlo se validan licencia, procedencia, versión, arquitectura y actualización de las herramientas incluidas en ese target.
- El core no pasa contraseñas en argumentos visibles de proceso ni escribe el SQL de la copia en registros. Si usa un archivo temporal para transferir credenciales, restringe su acceso, limita su vida y lo elimina al terminar o recuperarse de un cierre inesperado.
- La salida del proveedor se cifra como flujo hacia un artefacto temporal. Un archivo sin cifrar con el contenido de la base no queda persistido como resultado intermedio.

## Decisión de arquitectura

- El proveedor protegido se ejecutará dentro del core y usará el transporte ya establecido para la conexión. Un proceso `mysqldump`/`mariadb-dump` abriría otra conexión y el SSH con TLS identidad podría validar `127.0.0.1` en vez del host configurado; no se degradará TLS ni se omitirá el criterio 6 para habilitar una copia.
- Se conserva la investigación de CLI solo como referencia para exportación SQL o diagnóstico offline. Las operaciones protegidas usarán el adaptador de protocolo con el que DBSUAL ya verificó host, TLS y túnel.
- Para MySQL 8.0.3 o posterior, la captura interna primero valida familia y versión, luego usa `LOCK INSTANCE FOR BACKUP` con espera máxima de diez segundos y requiere `BACKUP_ADMIN`; después inicia `REPEATABLE READ` con `WITH CONSISTENT SNAPSHOT, READ ONLY` y lee catálogo, DDL y filas por la misma sesión. Las versiones anteriores y MariaDB se rechazan antes de intentar el bloqueo. La prueba MySQL Community 8.4.11 como root verificó que una segunda sesión no puede añadir una columna mientras el bloqueo está activo. Otra cuenta dedicada completó inventario, lectura de DDL y captura cifrada con `BACKUP_ADMIN` y `SHOW_ROUTINE` globales y `SELECT`, `SHOW VIEW`, `TRIGGER` y `EVENT` sobre la base de prueba. Este conjunto es evidencia inicial, no una declaración de permisos mínimos para cada categoría/version; falta probar todas las versiones admitidas. Si no puede tomar el bloqueo, se cancela.
- `vault::encrypt_artifact_reader_to_directory` cifra el flujo NDJSON en un artefacto autenticado sin sobrescribir archivos existentes, con SHA-256 del ciphertext. El historial publica puntos `captured` con cobertura parcial `visible_tables_views_triggers`; el restaurador autentica hash, tamaño e IDs antes de crear destino. La restauración admite tablas, vistas locales y triggers simples de una instrucción con referencias locales. El formato interno no es SQL portable y no es recuperación completa: faltan rutinas, funciones, eventos, demostración de visibilidad total y matriz de permisos/versiones.
- `inspect_mysql_backup_catalog` enumera objetos visibles (tablas, vistas, rutinas, triggers y eventos), charset y collation; informa si todas las tablas visibles son InnoDB y devuelve también el DDL renderizado por el servidor. La integración contra MySQL Community 8.4.11 verificó tabla, vista, trigger, procedimiento, evento y la detección de MyISAM, además de leer su DDL (2026-09-29). Estas lecturas no demuestran que la cuenta vea todos los objetos ni que el catálogo y filas compartan una instantánea. El flujo NDJSON actual conserva el DDL, pero no lo convierte en un script SQL portable.
- El contenido interno combina metadatos de la base, definiciones, filas de tablas base y conteos en NDJSON. Cada encabezado de tabla incluye su `CREATE TABLE` y marca las columnas generadas; los valores aún conservan los bytes del protocolo de texto en hex y `NULL` se representa aparte. Una canalización limitada a dos fragmentos de 64 KiB conecta SQLx con el cifrado, sin archivo intermedio en claro. La prueba verifica binarios `00/FF`, Unicode, `NULL`, texto literal `\\N`, tabla vacía, DDL de tabla y metadatos de columna generada, round-trip del artefacto, bloqueo efectivo ante DDL concurrente, captura con una cuenta dedicada y rechazo de una tabla MyISAM sin publicación (MySQL Community 8.4.11, 2026-09-29). Una cuenta a la que faltaba `SHOW VIEW` recibió el rechazo `1142` y no dejó artefacto publicado. Aún faltan permisos validados por versión y demostrar cobertura total del catálogo. El contenido no es SQL restaurable y no se considera un punto de recuperación válido.
- El restaurador autentica el ciphertext antes de crear destino, lo vuelve a descifrar por canal acotado, inserta filas con binds y comprueba conteos y huellas SHA-256 de conjuntos de filas. La integración MySQL Community 8.4.11 probó tablas, claves foráneas circulares, columna `INVISIBLE`, dos vistas locales dependientes y un trigger simple `AFTER INSERT`. El trigger se instala después de restaurar filas: el conteo copiado en la tabla de auditoría coincide con el origen (sin disparos duplicados) y un INSERT posterior confirma que la definición funciona en el destino. El DDL se compara con `SHOW CREATE TRIGGER`; el `DEFINER` se valida antes de crear destino. Se rechazan triggers con cuerpos compuestos, orden explícito `FOLLOWS/PRECEDES`, referencias no locales, definidores distintos de `CURRENT_USER()` o sintaxis no reconocida. Rutinas, funciones y eventos siguen fuera de alcance. La cobertura `visible_tables_views_triggers` permanece `captured`, nunca `verified`; faltan MySQL 8.0 y toda la matriz MariaDB, permisos mínimos y cobertura total. No es una copia completa según este spec.
- La captura cifrada del historial registra `captured` con cobertura parcial `visible_tables_views_triggers`. Las vistas requieren dependencias locales completas y acíclicas; los triggers requieren un cuerpo simple, referencias locales y un `DEFINER` disponible. La interfaz describe este alcance y permite restaurar en una base nueva; el comando autentica hash, tamaño e IDs antes de crearla y elimina el destino ante un flujo inválido. Falta verificar MySQL 8.0, permisos mínimos y cobertura de objetos; esto no es recuperación completa.
- Se añadió la integración ignorada `mysql_recovery_snapshot_round_trips_to_a_new_database`, configurable por `DBSUAL_MYSQL_TEST_*` y `DBSUAL_MYSQL_TEST_EXPECTED_VERSION`, con fixtures de tablas, FK, vista y trigger simple; comprueba filas, DDL y que el punto conserve estado `captured`. `cargo check --tests --locked` y la compilación/enlace del test pasaron en Windows el 2026-09-30. El primer intento contra MySQL 8.4.11 no pudo conectar porque el puerto desechable 33081 estaba cerrado (`CONNECTION_TIMEOUT`); la ejecución posterior contra 8.0.46 se documenta abajo. Esto no habilita destrucción ni declara respaldo completo.
- El inventario serializa para vistas, rutinas, triggers y eventos el `DEFINER`; para rutinas, triggers y eventos también guarda `SQL_MODE`, `CHARACTER_SET_CLIENT`, `COLLATION_CONNECTION` y `DATABASE_COLLATION`. Procedimientos, funciones y eventos todavía se rechazan antes de crear destino. La prueba real 8.4.11 comprueba restauración del trigger simple y su ejecución posterior.
- Se evaluó admitir procedimientos cuyo cuerpo parece una única sentencia `SELECT`. Se mantienen rechazados: en MySQL una sentencia `SELECT` puede invocar funciones con efectos, asignar variables, escribir archivos, adquirir bloqueos o consultar esquemas externos. Un filtro léxico no demuestra que el cuerpo sea de solo lectura ni que sus dependencias queden cubiertas por la copia. Para reconsiderarlo se necesita análisis que reconozca la gramática y el contexto de seguridad de cada versión, junto con una política verificable para funciones invocadas y referencias fuera de la base restaurada. Las pruebas conservan el rechazo de `SELECT 1` y de ejemplos con efectos; `FUNCTION` y `EVENT` también permanecen fuera de alcance.
- La prevalidación de versión acepta desde MySQL 8.0.3 y falla antes del bloqueo en versiones anteriores o en MariaDB. Pruebas unitarias cubren 8.0.2, 8.0.3, 8.0.x de proveedor, 8.4, una futura versión mayor y valores malformados.
- Investigación y sonda de `BACKUP STAGE` MariaDB: la función existe desde 10.4 (incluye 10.6, 10.11 y 11.4) y requiere el privilegio global `RELOAD`. `BLOCK_DDL` impide DDL mientras la etapa está activa; `BLOCK_COMMIT` detiene commits globales hasta `BACKUP STAGE END`. El test ignorado `mariadb_backup_stage_can_release_commit_block_after_snapshot` pasó en contenedores desechables MariaDB 10.6.28, 10.11.19 y 11.4.13 el 2026-09-29. Con una sesión dedicada a la etapa y otra a `START TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY`, comprueba que: (a) una escritura InnoDB queda bloqueada mientras `BLOCK_COMMIT` está activo; (b) termina después de `BACKUP STAGE END`; (c) la transacción conserva su vista previa; y (d) un `ALTER TABLE` en una tabla ya leída espera hasta que esa transacción termina. La variante inicial de una sola sesión no conservó la instantánea tras `BACKUP STAGE END`, por lo que se descartó. Esta sonda de una tabla InnoDB demuestra solo una estrategia candidata; no captura/restaura una base, no mide cargas largas ni cancela, y no cubre altas concurrentes de tablas, todas las dependencias de catálogo, permisos mínimos, fallos/interrupciones ni el resto de motores de tabla. Antes de habilitar el proveedor se requieren esas pruebas y la verificación de un artefacto restaurado. Fuentes: documentación oficial de [BACKUP STAGE](https://mariadb.com/docs/server/reference/sql-statements/administrative-sql-statements/backup-commands/backup-stage) y [MariaDB 10.4](https://mariadb.com/docs/release-notes/community-server/old-releases/10.4/what-is-mariadb-104).

## Coherencia de la copia

1. El proveedor inspecciona motores de tablas y objetos que debe cubrir, además de permisos para la estrategia de copia necesaria.
2. Para tablas transaccionales compatibles, se evalúa una instantánea coherente. También se controla la posibilidad de DDL concurrente con el mecanismo admitido por la versión del servidor y los permisos disponibles.
3. Si hay tablas no transaccionales, se requiere una estrategia de bloqueo que garantice coherencia para ellas. Si no puede obtenerse sin riesgo de copia inconsistente, no se crea un punto de recuperación «completo».
4. La interfaz informa cuando el respaldo puede bloquear escrituras o cambios de estructura y permite cancelar antes de adquirir el bloqueo.
5. Si se pierde conexión, falla el proveedor, cambia el esquema durante el proceso o no se puede verificar la integridad, se descarta el artefacto temporal y no se aplica la operación protegida.

MySQL documenta que una lectura `REPEATABLE READ` mantiene la misma vista InnoDB en la transacción, y que `LOCK INSTANCE FOR BACKUP` permite DML pero bloquea operaciones de archivos que podrían invalidar una copia; este bloqueo requiere `BACKUP_ADMIN`. DBSUAL implementa ambas precondiciones en la ruta interna, pero todavía debe demostrar el bloqueo bajo DDL concurrente y comprobar el efecto de espera con cargas reales. MariaDB se valida por separado.

## Verificación del artefacto

- Se exige finalización exitosa del proveedor, archivo completo, hash correcto y lectura del inventario de objetos guardados.
- El inventario se compara con el catálogo inspeccionado antes de la copia; las diferencias se explican y bloquean su uso como punto protegido cuando indican contenido faltante.
- Cada versión admitida pasa una prueba de restauración en una base aislada con datos, vistas, disparadores, rutinas, eventos, claves, valores binarios y Unicode representativos.
- La verificación por hash e inventario detecta ciertos fallos, pero no sustituye una restauración real. La interfaz no promete certeza absoluta de recuperación para toda configuración del servidor.

## Restauración

1. Por defecto, se restaura en una base nueva cuyo nombre elige la persona; no se sobrescribe la base original al probar una recuperación.
2. El proveedor comprueba familia y versión de destino, permisos, existencia del nombre y espacio disponible.
3. Descifra el artefacto como flujo hacia el cliente de restauración compatible y registra el avance sin registrar filas ni secretos.
4. Al terminar, compara inventario y datos verificables con el manifiesto de la copia. Si la restauración falla, deja la base nueva marcada como incompleta y ofrece retirarla mediante una revisión separada.
5. Reemplazar una base existente usa el flujo y las confirmaciones del spec 010, incluido un punto de recuperación del estado actual.

## Criterios de aceptación

1. Una copia MySQL con tablas, vistas, disparadores, rutinas y eventos se restaura en una base nueva de una versión admitida con objetos y datos equivalentes dentro del alcance declarado.
2. Una copia MariaDB equivalente se restaura con su proveedor propio en 10.6, 10.11 y 11.4; no se envía al cliente MySQL por simple similitud de protocolo.
3. Si faltan privilegios para incluir una categoría requerida, el respaldo protegido falla y la operación destructiva no comienza.
4. Una base con tablas no transaccionales no se respalda bajo una estrategia que solo garantice coherencia de tablas transaccionales.
5. Una interrupción, error de herramienta o falta de espacio deja el artefacto incompleto sin marcar como punto válido y no inicia la operación destructiva.
6. La ejecución con TLS y, cuando esté configurado, túnel SSH usa la conexión prevista y no expone secretos en registros ni argumentos visibles.
7. El proveedor declara familia y versión del artefacto; la interfaz no ofrece restauración cruzada MySQL↔MariaDB sin compatibilidad validada.

## Decisiones pendientes

1. Versiones exactas de servidor cubiertas por pruebas del proveedor en proceso; no se distribuirán clientes CLI como requisito del backup.
2. Estrategia concreta de bloqueo para cada combinación de motor de tabla, versión y permisos.
3. Profundidad de verificación de datos de cada copia antes de una operación destructiva.

## Referencias técnicas

- [MySQL: opciones y límites de `mysqldump`](https://dev.mysql.com/doc/refman/8.4/en/mysqldump.html).
- [MySQL: bloqueo de instancia para respaldos](https://dev.mysql.com/doc/refman/8.4/en/lock-instance-for-backup.html).
- [MySQL: cambios de `LOCK INSTANCE FOR BACKUP` en 8.0.3](https://dev.mysql.com/doc/relnotes/mysql/8.0/en/news-8-0-3.html), [aislamiento de transacciones InnoDB](https://dev.mysql.com/doc/refman/8.4/en/innodb-transaction-isolation-levels.html).
- [MariaDB: `mariadb-dump` y diferencias de compatibilidad](https://mariadb.com/docs/server/clients-and-utilities/backup-restore-and-import-clients/mariadb-dump).
- [MariaDB: restauración de archivos de volcado](https://mariadb.com/docs/server/mariadb-quickstart-guides/mariadb-restore-guide).

## Implementación parcial MariaDB — 2026-09-29

- El historial ofrece captura y restauración en una base nueva para conexiones MariaDB, separado de los formatos de artefacto MySQL. El core valida familia del servidor y limita la operación a las ramas 10.6, 10.11 y 11.4; al restaurar exige la misma rama que creó el artefacto. Un formato MariaDB no se acepta en el restaurador MySQL ni a la inversa.
- El proveedor MariaDB usa dos sesiones: una controla `BACKUP STAGE` y la otra mantiene la lectura consistente InnoDB. Tras inventariar y adquirir los bloqueos de metadatos necesarios, libera la etapa de respaldo antes de serializar filas. Si no puede asegurar el inventario/dependencias o liberar la etapa de forma inequívoca, falla cerrado y descarta el artefacto provisional.
- La cobertura sigue siendo `visible_tables_views_triggers`: tablas InnoDB, vistas con dependencias locales reconocidas y triggers locales de cuerpo simple. Rutinas, funciones, eventos, motores de tabla no transaccionales, visibilidad total del catálogo y permisos mínimos siguen excluidos; esta ruta no habilita aplicar modificaciones destructivas.
- Se añadió una prueba Playwright del recorrido visual MariaDB en historial: conexión simulada, invocación tipada de captura y diálogo de restauración con etiqueta y motor correctos. Es evidencia de UI/IPC simulado, no del comando Tauri nativo.
- La suite habitual del 2026-09-29 pasó: 58 pruebas Rust; siete integraciones quedaron ignoradas por requerir servidor desechable; 13 pruebas E2E pasaron incluyendo el flujo simulado MariaDB; build de producción pasó. Intenté repetir la prueba real desde el proceso Windows contra un contenedor Docker local, pero el cliente de pruebas no logró conectarse; ese intento no se considera prueba del proveedor. La ronda de integración Linux desechable registrada anteriormente para MariaDB 10.6.28, 10.11.19 y 11.4.13 ejercitó captura/restauración del proveedor antes de los últimos cambios de IPC/prevalidación de rama; se debe repetir la matriz con el recorrido final.
- Por tanto, los criterios 1–7 del spec siguen abiertos para la definición de respaldo **completo**. La compatibilidad parcial del proveedor y el panel no cambian el estado `captured` a `verified` ni cumplen la puerta de recuperación para operaciones destructivas.

## Revalidación Windows — 2026-09-30

- La integración `mariadb_recovery_snapshot_round_trips_to_a_new_database` pasó en Windows contra MariaDB 10.6.28, 10.11.19 y 11.4.13, cada una en un contenedor desechable. Verificó captura cifrada y restauración a una base nueva de tres tablas, dos filas, relación foránea, Unicode, binarios, `NULL`, vista local y trigger simple; comprobó que el trigger no se ejecutara durante la carga y que funcionara después. También creó y retiró la clave temporal de la prueba en el almacén de credenciales de Windows.
- Esta integración llama al proveedor/core, no al comando Tauri ni al recorrido de interfaz. La cobertura continúa parcial (`visible_tables_views_triggers`): no incluye rutinas, funciones, eventos, tablas no transaccionales ni permisos mínimos; no convierte los respaldos en `verified` ni habilita operaciones destructivas.

## Límite de captura parcial — 2026-09-30

- Se amplió la integración desechable MySQL para añadir una rutina no soportada a una base que antes podía capturarse y comprobar que la siguiente captura falla sin publicar un artefacto cifrado. Esto verifica el rechazo ante una categoría omitida y evita que un punto incompleto se presente como válido.
- Esta evidencia cubre MySQL Community 8.4.11 únicamente y está incorporada a la integración ignorada que requiere un servidor desechable. No comprueba toda la visibilidad del catálogo, la restauración de rutinas ni el recorrido Tauri. Un snapshot de tablas, vistas locales y triggers simples permanece `captured`; `verified` sigue reservado para la cobertura completa, matriz de versiones/permisos y pruebas de restauración exigidas por este spec. No se habilita destrucción.

## Revalidación del round-trip parcial en Windows — MySQL 8.0.46 (2026-09-30)

- `mysql_recovery_snapshot_round_trips_to_a_new_database` pasó contra MySQL Community 8.0.46 en un servidor temporal aislado en WSL, accesible desde el proceso de prueba Windows. Capturó y restauró tres tablas InnoDB, dos filas, una FK, una vista y un trigger simple; los datos incluyeron Unicode, bytes binarios y `NULL`. La prueba comparó DDL y datos, verificó que el trigger no se reprodujera al restaurar y que funcionara después, y confirmó que el punto continúa `captured`. Después creó un procedimiento no admitido e hizo una segunda captura: el proveedor la rechazó sin publicar un artefacto para un snapshot incompleto.
- El mismo round-trip pasó en MySQL Community 8.4.11 con dos triggers `BEFORE INSERT` sobre la misma tabla: se crearon en orden `z_append_a`, `a_append_b`, opuesto al orden alfabético. La prueba comprobó `ACTION_ORDER` en origen y destino y verificó que la inserción restaurada produjera `post restoreAB`. Esto valida el orden implícito que hoy captura y reproduce el formato; no cubre `FOLLOWS/PRECEDES`, rutinas, eventos, permisos mínimos ni la matriz completa. El punto sigue `captured`.
- La restauración parcial también pasó contra 8.0.46; junto con las integraciones previas en 8.4.11, demuestra esta ruta limitada en ambas ramas. El proveedor sigue sin cubrir rutinas, funciones, eventos, tablas no transaccionales ni permisos mínimos; no es respaldo completo y no habilita operaciones destructivas ni cambia `captured` a `verified`.
