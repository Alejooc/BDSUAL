# Spec 010 — Respaldos y recuperación

Estado: diseño en implementación; proveedor MySQL parcial, otros motores pendientes
Versión: 0.8
Última actualización: 2026-09-30
Depende de: [006 — Control de cambios](006-control-de-cambios.md), [008 — Cuadrícula de datos](008-cuadricula-datos.md), [009 — Arquitectura](009-arquitectura.md)

## Objetivo

Definir qué protección se exige antes de aplicar cada revisión y cómo se recupera un estado anterior. El historial y los respaldos del primer MVP se guardan localmente en la plataforma de escritorio activa; rutas, almacén seguro y empaquetado por OS se rigen por los specs 014 y 019.

## Niveles de recuperación

| Nivel | Qué conserva | Qué permite |
| --- | --- | --- |
| Descartar borrador | Plan todavía no aplicado | Eliminar la intención de cambio sin tocar la base. |
| Reversión transaccional | Transacción aún abierta y motor compatible | Cancelar los efectos antes de confirmar la transacción. |
| Compensación por cambio | Identidad de las filas y valores anteriores o resultado de inserción | Preparar una nueva revisión inversa cuando el destino no cambió de forma incompatible. |
| Restauración completa | Respaldo verificado de estructura y datos de la base | Crear una base recuperada o reemplazar un destino tras una confirmación específica. |

La interfaz muestra el nivel disponible para cada revisión. **Compensación por cambio** no se presenta como restauración completa: disparadores, cascadas, rutinas u otros efectos indirectos pueden impedir que la nueva revisión reproduzca exactamente el estado anterior.

## Clasificación antes de aplicar

- **Cambio localizado:** inserción o actualización de filas identificables cuando el adaptador puede capturar valores anteriores, comprobar conflictos y determinar que el alcance es conocido. Se guarda la información de compensación cifrada. No se crea una copia completa por defecto.
- **Operación destructiva:** eliminar una base, tabla, columna o filas; truncar; reemplazar datos existentes; restaurar sobre un destino; importar SQL que modifica una base existente. Exige respaldo completo del destino antes de aplicar.
- **Efecto incierto:** SQL arbitrario, procedimientos, disparadores, cascadas o cualquier operación cuyo alcance el adaptador no pueda demostrar. Exige respaldo completo antes de aplicar.
- **Creación sin estado anterior:** crear una base nueva no requiere respaldo anterior, pero se registra su inexistencia previa y la identidad exacta del nuevo objeto. Una compensación posterior debe revisar si recibió datos o cambios externos.

El core clasifica la revisión completa según su operación más exigente. Si no puede determinar si un cambio localizado tiene efectos indirectos, usa **Efecto incierto**. La persona no puede rebajar manualmente el nivel exigido para omitir un respaldo.

## Creación del respaldo completo

1. Antes de aplicar, el proveedor del motor identifica la base exacta, versión, objetos incluidos y condiciones necesarias para una copia coherente.
2. Comprueba permisos y capacidad local disponible; informa tamaño y tiempo estimados cuando sea posible.
3. Genera el respaldo en un archivo temporal cifrado. El contenido no pasa por la interfaz como un único mensaje.
4. Finaliza el archivo, calcula su hash y verifica que el proveedor puede leer el inventario del respaldo. Registra motor, versión, fecha, tamaño, hash y revisión que protege.
5. Solo entonces permite aplicar la operación. Si falla cualquiera de estos pasos, la revisión permanece sin aplicar y se explica el bloqueo.

La verificación del archivo comprueba integridad y formato; no sustituye las pruebas periódicas de restauración. Cada proveedor de motor debe pasar una restauración automatizada con estructura y datos representativos antes de habilitar operaciones destructivas en ese motor.

## Restaurar

1. La persona elige un punto de recuperación. DBSUAL muestra qué base y fecha representa, qué datos actuales cambiarían y si el artefacto está disponible e íntegro.
2. Por defecto, la restauración crea una base o archivo nuevo para inspeccionar el resultado antes de reemplazar un destino existente.
3. Reemplazar una base existente requiere una revisión separada, una copia del estado actual y confirmación explícita del destino exacto.
4. Si el destino cambió desde la selección del punto, se detiene el reemplazo y se vuelve a revisar.
5. El resultado de la restauración se verifica y se registra como un nuevo evento. El historial anterior permanece.

Restaurar puede requerir permisos y espacio adicionales. Una copia local no protege frente a la pérdida del computador o de su clave de cifrado; la interfaz debe decirlo cuando presente opciones de recuperación.

## Retención y claves

- Un punto de recuperación necesario para una revisión aplicada no se elimina silenciosamente.
- Antes de borrar un respaldo, la interfaz muestra qué revisiones perderían la opción de restauración completa.
- Los respaldos y las imágenes de datos anteriores se cifran en reposo. Las claves legibles no se guardan dentro del SQLite de metadatos ni junto a los respaldos; las envolturas cifradas y el formato portable se definen en [014 — Claves y artefactos cifrados](014-claves-y-artefactos-cifrados.md).
- La clave de cifrado local se protege mediante el almacén seguro nativo de la plataforma. La persona dispone de dos vías independientes para recuperarla tras reinstalar o cambiar de sistema: una **frase de recuperación** y un **archivo de clave exportable** protegido con una contraseña elegida al exportarlo.
- DBSUAL guía a la persona para guardar la frase y el archivo fuera de la carpeta de datos de la aplicación. Confirma que al menos una vía de recuperación se configuró antes de crear el primer respaldo cifrado.
- Importar una frase o un archivo de clave recupera el acceso a artefactos anteriores sin modificarlos. Una contraseña o frase incorrecta falla sin alterar el historial ni los respaldos.
- Si el almacén de credenciales no está disponible y no se puede acceder a la clave, la aplicación mantiene disponibles las lecturas que no la necesiten, pero bloquea las modificaciones que exijan un punto de recuperación.
- En el primer MVP no se borran respaldos automáticamente. La persona puede liberar espacio manualmente después de ver qué revisiones perderían restauración completa. Si no hay espacio suficiente para un respaldo requerido, la modificación se bloquea en lugar de borrar otros respaldos sin autorización.

## Criterios de aceptación

1. Una actualización de fila identificable y sin efectos indirectos conocidos guarda valores anteriores y ofrece una revisión compensatoria, sin generar por defecto una copia completa.
2. Una eliminación de fila o base exige un respaldo completo. Si el respaldo falla, no se envía la eliminación al motor.
3. SQL cuyo alcance no puede demostrarse exige respaldo completo y muestra esa condición antes de aplicar.
4. Una copia con hash incorrecto o inventario ilegible no se ofrece para restauración.
5. Restaurar a un destino nuevo conserva la base original y registra el resultado en el historial.
6. Reemplazar una base existente requiere un nuevo punto del estado actual y confirmación explícita; si el destino cambió, se bloquea.
7. Si falta una clave o artefacto local, el historial sigue visible pero la restauración correspondiente se marca como no disponible.
8. Tras reinstalar Windows, macOS o Linux, una frase válida recupera el acceso a un respaldo cifrado sin requerir el perfil anterior.
9. Tras reinstalar o cambiar entre Windows, macOS y Linux, un archivo de clave exportado y su contraseña válida recuperan el acceso al mismo respaldo sin requerir la frase ni el almacén local anterior.
10. Una frase o contraseña incorrecta no modifica el respaldo, el historial ni la base de destino.

## Decisiones pendientes

1. Mecanismo y formato de respaldo por motor, junto con las versiones compatibles.
2. Biblioteca y formato binario exacto del cifrado y del archivo exportable, sujetos a las pruebas del spec 014 antes de implementar.
3. Margen de espacio libre necesario para iniciar y completar un respaldo sin agotar el disco.

## Evidencia de implementación

- El depósito cifra artefactos en bloques autenticados. Se añadió una API que recibe un `Read`, escribe a un archivo temporal, verifica el artefacto cifrado antes de publicarlo sin sobrescritura y devuelve tamaño más SHA-256 del ciphertext. Una prueba procesa varias ventanas de 64 KiB, restaura el contenido y comprueba hash y ausencia de texto claro en el archivo publicado.
- El catálogo MySQL ya lista tablas, vistas, rutinas, triggers y eventos visibles y distingue motores distintos de InnoDB. La prueba en MySQL 8.4.11 confirma esta clasificación; aún no prueba consistencia ni integridad de una copia de base real.
- El proveedor MySQL/MariaDB tiene captura NDJSON cifrada y restauración parcial de tablas, vistas locales y triggers simples, conectadas a IPC e historial. La restauración solo crea una base nueva. Preparar guarda un plan cifrado ligado al punto y destino; confirmar no ejecuta la restauración; aplicar exige estado confirmado, vuelve a verificar el punto y el nombre libre, y registra `applied`, `failed` o `uncertain` según lo observado del servidor. Playwright cubre el flujo UI/IPC simulado; falta probar la ruta Tauri contra servidores MySQL y MariaDB reales. Cada tabla lleva una huella SHA-256 de conjunto que distingue NULL, bytes y duplicados sin depender del orden de lectura; después de insertar, DBSUAL vuelve a leer la tabla destino y exige la misma huella. Las claves foráneas se agregan después de crear y cargar todas las tablas, lo que permite referencias circulares. Los triggers se instalan después de cargar los datos para evitar efectos duplicados y el destino se limpia si falla la restauración. MySQL 8.4 pasó pruebas reales de estas categorías. Rutinas, funciones y eventos siguen rechazados; en particular, un procedimiento con cuerpo `SELECT` no se acepta sin analizar de manera completa sus efectos y dependencias. Aún falta validar la matriz completa de versiones y permisos/visibilidad; esto sigue siendo parcial y no habilita aplicación de cambios destructivos. PostgreSQL y SQLite tampoco tienen proveedor.
- La migración 5 del SQLite local guarda metadatos para puntos de recuperación: motor y versión, ID y hash del ciphertext, tamaños, cobertura, estado de verificación y revisión protegida. El comando de historial puede capturar y registrar un artefacto MySQL, y la interfaz muestra su estado y cobertura. El restaurador parcial admite tablas, vistas locales y triggers simples; rechaza rutinas, funciones y eventos. El punto queda como `captured`, con cobertura parcial; no se considera verificado ni habilita aplicación/destrucción. Un procedimiento de una sola sentencia `SELECT` también permanece rechazado porque el texto puede invocar funciones con efectos, escribir archivos o variables, adquirir bloqueos y acceder a esquemas externos, y el proveedor no analiza esas dependencias de forma completa.
- El core puede autenticar un artefacto con la clave recuperada del almacén seguro de la plataforma y escribir el contenido completo hacia un sumidero, validando todos los tags y el cierre antes de que el llamador cree un destino. La prueba actual de Windows rechaza un cierre alterado. El consumidor de streaming aún no está integrado y no se afirma restauración real.
- Se añadió compensación cifrada para una edición MySQL localizada: solo admite columna no clave de tabla InnoDB con PK y sin triggers, compara la fila al preparar y aplicar, y conserva la preimagen como artefacto autenticado. Preparar una reversión valida que la revisión fuente figure como aplicada, vuelve a comparar el valor actual con el valor aplicado y crea un nuevo borrador cuyo valor propuesto es el anterior. La integración del core pasó contra MySQL Community 8.4.11 desechable para edición, compensación y conflicto. La revisión local usó un artefacto cifrado real, historial SQLite aislado y el lector autenticado compartido con `read_history_plan`; la aplicación de compensación se prepara directamente en la prueba y no cubre `prepare_mysql_row_revert`. El comando Tauri con `AppHandle` ni el flujo de reversión desde IPC no se invocaron. Esto no sustituye el respaldo completo para operaciones destructivas.
- La integración MySQL Community 8.4.11 desechable también comprueba el límite de captura parcial: una base con una rutina fuera de cobertura hace fallar `encrypt_mysql_table_recovery_snapshot` y no deja ciphertext publicado. La captura válida previa continúa en `captured`; esta prueba de rechazo no valida cobertura completa ni habilita operaciones destructivas.

## Referencias técnicas

### Inserción protegida MySQL y recuperación localizada — 2026-09-30

El plan cifrado de inserción conserva todos los valores explícitos y la condición inicial de PK libre. Revertir prepara una revisión nueva de eliminación solo cuando la lectura de la fila confirma que todas sus columnas siguen iguales; aplicar repite la huella bajo bloqueo y rechaza triggers o FKs entrantes. La integración del core pasó contra MySQL Community 8.4.11 desechable para insertar, verificar, preparar la compensación y eliminar, autenticando los artefactos con el lector del historial. Fixtures reales probaron que un trigger `BEFORE INSERT` y una FK entrante `ON DELETE CASCADE` bloquean inserción y compensación; una escritura externa hizo fallar la preparación compensatoria y conservó su fila/valor. Playwright recorrió la secuencia con IPC simulado. No se recorrieron los comandos Tauri ni el IPC nativo. Estos flujos no reemplazan el respaldo completo y no habilitan eliminación general.

- [MySQL: condiciones de consistencia de `mysqldump --single-transaction`](https://dev.mysql.com/doc/refman/8.4/en/mysqldump.html).
- [MariaDB: respaldo con `mariadb-dump`](https://mariadb.com/docs/server/clients-and-utilities/backup-restore-and-import-clients/mariadb-dump).
- [PostgreSQL: respaldo y restauración](https://www.postgresql.org/docs/current/backup.html).
- [SQLite: API de respaldo en línea](https://www.sqlite.org/backup.html).
