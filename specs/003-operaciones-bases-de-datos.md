# Spec 003 — Crear y eliminar bases de datos

Estado: creación MySQL en validación; eliminación pendiente
Versión: 0.7
Última actualización: 2026-09-29  
Depende de: [001 — Conexiones](001-conexiones.md), [002 — Explorador](002-explorador.md)

Todas las acciones de creación y eliminación se preparan y aplican mediante el flujo de [006 — Control de cambios](006-control-de-cambios.md). Si el historial no está disponible, no se ejecutan.

## Objetivo

Permitir crear y eliminar bases de datos desde una conexión abierta, con una confirmación clara para la operación destructiva y un resultado visible en el explorador. MySQL se implementará primero, seguido de MariaDB, PostgreSQL y SQLite.

## Crear una base de datos

### Flujo común

1. La persona elige **Crear base de datos** desde una conexión compatible.
2. Introduce el nombre o la ruta que corresponda al motor.
3. La interfaz valida los datos y prepara una revisión para crear la base. Confirmarla no crea todavía la base.
4. Al elegir **Aplicar a la base**, la aplicación vuelve a comprobar el destino y ejecuta la creación. Mientras se ejecuta, muestra progreso o estado de espera y evita envíos duplicados.
5. Al terminar correctamente, registra el resultado, actualiza el explorador y señala la nueva base de datos.
6. Si falla, registra la causa y conserva la revisión para poder corregirla o inspeccionar el estado real.

### Reglas por motor

- **MySQL:** se solicita un nombre. El motor usa sus valores predeterminados para las opciones no configuradas en este flujo. La operación requiere los permisos correspondientes.
- **MariaDB:** se solicita un nombre y se aplican sus reglas y permisos propios. La implementación no reutiliza SQL específico de MySQL sin pruebas en MariaDB.
- **PostgreSQL:** se solicita un nombre. La creación se ejecuta desde una conexión apta para administrar otras bases de datos, no desde la base que se intenta crear. La operación requiere los permisos correspondientes.
- **SQLite:** se solicita una ubicación y nombre de archivo. La aplicación crea un archivo de base de datos nuevo y registra una conexión a él. Nunca reemplaza un archivo existente de forma silenciosa.

## Eliminar una base de datos

### Flujo común

1. La persona elige **Eliminar base de datos** desde el nodo de una base de datos y prepara una revisión. Preparar o confirmar la revisión no elimina nada.
2. Antes de **Aplicar a la base**, la vista previa muestra motor, conexión y nombre o ruta exacta del destino. Explica que se perderán el esquema y los datos y muestra el estado del punto de recuperación.
3. Para confirmar la aplicación, la persona escribe el nombre visible de la base de datos. El botón de eliminación permanece desactivado hasta que coincida.
4. Durante la operación se impide repetir la solicitud.
5. Si termina correctamente, la base de datos desaparece del explorador, se limpia cualquier selección dependiente y se registra el éxito en el historial.
6. Si falla, se indica la causa conocida y se vuelve a inspeccionar el destino antes de concluir su estado.

### Reglas por motor

- **MySQL:** la eliminación se envía al servidor de la conexión. La aplicación no intenta eliminar una base de datos para la que no tenga permisos; si el servidor rechaza la operación, muestra el error.
- **MariaDB:** la eliminación se envía al servidor MariaDB y se registra bajo su propio adaptador y sus propias pruebas de recuperación.
- **PostgreSQL:** la aplicación no intenta eliminar la base de datos desde una sesión conectada a ella. Si hay sesiones activas u otra restricción del motor, informa el bloqueo y no fuerza el cierre de sesiones ajenas sin una acción especificada aparte.
- **SQLite:** **Eliminar base de datos** borra el archivo local después de confirmar su ruta exacta y de crear el respaldo exigido. Antes, la aplicación cierra sus conexiones y deja que SQLite consolide o retire sus archivos auxiliares mediante sus propias API. Si hay un diario o WAL que puede contener datos pendientes de consolidación, o si otra aplicación mantiene el archivo abierto, se bloquea la eliminación y se explica el motivo.
- DBSUAL no borra manualmente un archivo `-wal` para forzar la eliminación. Solo retira el archivo principal cuando la base se cerró de forma segura y su ruta exacta se verificó; no hace un borrado recursivo ni elimina archivos de otro nombre.

## Reglas generales

- Quitar una conexión guardada y eliminar una base de datos son acciones distintas, con textos y controles distintos.
- El core construye la operación usando las reglas del motor y valida y cita los identificadores según corresponda. Los valores introducidos por la persona no se interpolan como SQL sin escape.
- Al crear, un nombre ya existente o una ruta ocupada produce un error claro; no se sobrescribe ni se adopta silenciosamente el objeto existente.
- Al eliminar, el explorador se actualiza con el estado confirmado por el motor. Si no se puede confirmar el resultado tras una interrupción, se solicita actualizar antes de mostrar una conclusión.
- Las acciones solo se ofrecen cuando la conexión está abierta y el motor admite la operación. Los permisos definitivos los decide el motor; la interfaz no interpreta una lista incompleta de permisos como garantía de éxito.
- La aplicación no elimina archivos auxiliares o rutas distintas de la base de datos seleccionada sin una regla explícita en el spec específico del motor.

## Criterios de aceptación iniciales: MySQL

1. Dada una conexión MySQL abierta con permiso de creación, al preparar y confirmar una revisión la base aún no existe; tras **Aplicar a la base** con éxito aparece en el explorador.
2. Dado un nombre inválido o ya existente, la creación no sobrescribe ninguna base de datos y se informa el problema.
3. Dada una cuenta sin permiso suficiente, la creación o eliminación muestra el rechazo del servidor y el explorador no afirma que se completó.
4. Dada una base de datos seleccionada para eliminar, la confirmación identifica la conexión y la base de datos exactas; sin escribir el nombre no se habilita la acción.
5. Dada una eliminación aplicada y aceptada por el servidor, la base de datos desaparece del explorador y las selecciones dependientes se limpian.
6. Dada una eliminación fallida o interrumpida, la interfaz no muestra un éxito falso y permite actualizar para verificar el estado real.
7. Quitar la conexión MySQL no ejecuta la eliminación de ninguna base de datos.

## Evidencia de implementación

- La interfaz permite preparar, revisar, confirmar y aplicar la creación de una base MySQL vacía. El borrador queda cifrado en el historial; confirmar no ejecuta SQL.
- El core limita el nombre a 64 caracteres ASCII seguros, registra el servidor MySQL (`@@server_uuid`) y su versión en el plan, comprueba que el destino siga igual y que el nombre siga libre antes de aplicar. Después vuelve a consultar el catálogo y registra aplicado, fallido o incierto según lo que pudo comprobar.
- La prueba de integración `mysql_create_database_revision_requires_confirmation_and_reconciles_conflicts` pasó el 2026-09-29 contra MySQL Community 8.4.11 en un contenedor desechable. Usa `connections::connect` con la misma construcción y validación de `SavedConnection` que la aplicación; confirma que preparar y confirmar no crean la base, que aplicar la crea y registra `applied`, y que una creación externa posterior a la confirmación bloquea la aplicación sin alterar sus datos. Limpia los dos nombres aleatorios y el historial temporal antes de propagar fallos o pánicos del cuerpo. Invoca los servicios compartidos y la transición del historial, no los comandos Tauri ni la actualización del explorador nativo; el criterio 1 sigue parcialmente verificado.
- Repetida el 2026-09-30 contra MySQL Community 8.4.11 desechable: `mysql_create_database_revision_requires_confirmation_and_reconciles_conflicts` pasó (1 prueba, 0 fallos). Se ejecutó con usuario `root`; confirma de nuevo el servicio y la detección de conflicto, pero no cambia el alcance de la evidencia: siguen pendientes el comando Tauri nativo, la actualización del explorador y una cuenta de permisos mínimos.
- Playwright simula Tauri IPC y recorre el formulario, la lectura del plan cifrado, confirmar sin aplicar, abrir la revisión confirmada y aplicar. Comprueba que la base no aparece antes de aplicar, que el estado final es `applied` y que el catálogo se vuelve a consultar y muestra el nuevo nombre. La prueba cubre la integración de UI con los contratos de comandos simulados; no ejecuta el binario Tauri ni sustituye la prueba del servidor MySQL.
- El comando de creación solo acepta revisiones confirmadas con recuperación `not_required` y engine MySQL; no es una vía general para ejecutar SQL. MariaDB, eliminación y SQLite siguen pendientes. PostgreSQL no se admite aún.

## Criterios de aceptación: SQLite

1. Al crear una base de datos SQLite en una ruta libre y válida, se crea un archivo nuevo y aparece una conexión a ese archivo en el explorador.
2. Si ya existe un archivo en la ruta elegida, la creación se detiene sin sobrescribirlo.
3. Antes de eliminar una base SQLite, la confirmación muestra la ruta exacta y requiere escribir el nombre del archivo.
4. Tras confirmar, la aplicación cierra la conexión de forma segura, deja a SQLite gestionar los archivos auxiliares y borra únicamente el archivo principal verificado. Si no puede garantizar el cierre y la consolidación, no borra nada.
5. Si algún paso falla, se muestra el error sin presentar la operación como completada y se puede verificar o reintentar el estado desde el explorador.
6. Quitar la conexión SQLite guardada no borra el archivo de base de datos.

## Fuera de este spec

- Crear, renombrar o eliminar tablas, vistas, columnas o esquemas.
- Configurar opciones avanzadas de creación como codificación, intercalación o plantillas.
- Restaurar una base de datos eliminada.
- Importar o exportar SQL y CSV.
- Cerrar forzosamente conexiones de terceros para poder eliminar una base de datos.

## Decisiones pendientes

1. Si se debe ofrecer una exportación previa desde el diálogo de confirmación de eliminación.
