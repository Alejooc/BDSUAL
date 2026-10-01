# Spec 009 — Arquitectura técnica del MVP

Estado: borrador para revisión  
Versión: 1.3  
Última actualización: 2026-09-28  
Depende de: [000 — Visión](000-vision-y-alcance.md), [006 — Control de cambios](006-control-de-cambios.md)

## Objetivo

Definir los límites entre interfaz, core, motores de base de datos, historial y respaldos para implementar un MVP de escritorio para Windows, macOS y Linux sin perder las garantías acordadas: toda modificación se prepara antes de aplicarse y las operaciones destructivas requieren un punto de recuperación verificado. Las APIs nativas quedan detrás de adaptadores por sistema operativo, según el spec 019.

## Componentes

| Componente | Responsabilidad |
| --- | --- |
| React y TypeScript | Presentar el espacio de trabajo, formularios, editor, cuadrícula y vistas previas; mantener solo estado de interfaz y borradores transitorios. |
| Tauri Commands / IPC | Exponer acciones tipadas entre la interfaz y Rust. Los trabajos largos notifican progreso y estado mediante un canal identificado por trabajo. |
| Core Rust | Validar destinos, permisos y precondiciones; coordinar conexiones, historial, respaldo, aplicación y verificación. Es la autoridad sobre el estado persistente. |
| Adaptadores de motor | Implementar metadatos, consultas, operaciones y reglas específicas de MySQL, MariaDB, PostgreSQL y SQLite detrás de contratos comunes. |
| Almacén local de metadatos | Guardar conexiones sin secretos, preferencias, proyectos, revisiones, trabajos y eventos de aplicación. |
| Almacén de artefactos | Guardar de forma cifrada archivos de entrada con datos, imágenes anteriores y respaldos completos; verificar integridad por hash. |
| Adaptador de credenciales del sistema | Guardar contraseñas y claves de cifrado en el almacén seguro nativo disponible en Windows, macOS o Linux; en SQLite solo quedan identificadores no secretos. |
| Integración Git opcional | Exportar cambios de estructura y scripts revisables cuando la persona la configure; no determina el estado real de las bases. |

## Decisiones de almacenamiento local

- Decisión del MVP: el historial y los respaldos viven en el equipo local y el perfil de usuario de la plataforma donde se creó el proyecto. La sincronización entre computadores queda para una fase posterior.
- Se elige **SQLite** para metadatos e historial local. Permite registrar revisiones, eventos y cambios de estado con transacciones y una evolución de esquema controlada. JSON queda fuera como almacén principal.
- Las preferencias simples pueden vivir en la misma base local. Las contraseñas, frases de paso y claves de cifrado no se guardan allí.
- Los respaldos grandes y otros artefactos se guardan como archivos separados en una ubicación de datos de la aplicación, cifrados y referenciados desde SQLite. No se guardan como blobs dentro de la base de metadatos.
- Cada artefacto tiene identificador, tipo, tamaño, hash, ruta gestionada, estado de verificación y relación con la revisión o el punto de recuperación.
- El historial es de solo anexado a nivel lógico: una aplicación, un fallo o una restauración agregan eventos; no reescriben una revisión aplicada.

## Contratos del core

La interfaz invoca acciones de alto nivel y recibe respuestas con identificadores y estados. No construye SQL de administración ni manipula directamente el archivo del historial.

| Contrato | Operaciones iniciales |
| --- | --- |
| Conexiones | Crear, probar, abrir, cerrar, editar y quitar; recuperar secretos solo en el core. |
| Catálogo | Listar bases, esquemas, tablas, vistas y columnas; consultar filas por lotes. |
| Consultas | Ejecutar lectura con destino explícito y devolver resultados por lotes. |
| Cambios | Preparar, previsualizar, confirmar revisión, aplicar y consultar historial. |
| Recuperación | Crear, verificar, listar y restaurar puntos de recuperación. |
| Transferencia | Exportar SQL/CSV; preparar y aplicar importaciones SQL/CSV. |
| Trabajos | Consultar progreso, cancelar cuando sea viable y reconciliar trabajos interrumpidos. |

Las llamadas incluyen identificadores de conexión, base de datos, proyecto y revisión según corresponda. El core vuelve a comprobar esos identificadores y el contexto real antes de ejecutar; no confía en un destino mostrado por la interfaz.

## Flujo de una modificación

1. La interfaz envía la intención al core. El core crea o abre el proyecto local del destino y guarda un borrador duradero.
2. El adaptador del motor calcula la vista previa y las precondiciones que puede comprobar. Los efectos desconocidos se declaran como tales.
3. Confirmar la revisión fija el plan y sus artefactos de entrada mediante hash. Aún no se envía una modificación al motor.
4. **Aplicar a la base** adquiere un bloqueo local por proyecto, vuelve a inspeccionar destino y precondiciones y registra un trabajo **Aplicando** antes de cualquier efecto remoto.
5. El servicio de recuperación crea y verifica la protección requerida: datos para compensación o respaldo completo, según el spec 010. Si falla, se registra el fallo y no se envía la modificación.
6. El adaptador ejecuta cada operación y registra sus resultados. Tras ejecutar, vuelve a inspeccionar el destino.
7. El historial termina en **Aplicado**, **Fallido**, **Fallido parcialmente** o **Estado incierto** según la evidencia. El bloqueo se libera y la interfaz actualiza explorador, pestañas e historial.

No existe una transacción atómica entre el SQLite local y una base remota. Por eso el trabajo y cada paso se registran de forma durable. Después de un cierre inesperado, DBSUAL reconcilia los trabajos incompletos consultando el destino antes de permitir una nueva aplicación sobre el mismo proyecto. El [spec 015](015-conflictos-y-aplicacion.md) define conflictos, resultados parciales y pasos inciertos.

## Adaptadores y respaldos

- SQLx se usa para conexiones, metadatos, consultas y operaciones SQL que admita cada motor. Las diferencias de sintaxis, permisos, tipos y transacciones quedan dentro de cada adaptador.
- Un proveedor de respaldo por motor se ocupa de crear, verificar y restaurar copias completas. Su implementación puede usar mecanismos nativos del motor; no se supone que SQLx produzca por sí solo un volcado completo y fiel.
- Para SQLite legible, el candidato de respaldo cifrado es `sqlcipher_export()` hacia un archivo SQLCipher. La Online Backup API no permite convertir directamente de fuente legible a destino cifrado. La prueba de consistencia y empaquetado del [spec 013](013-respaldos-sqlite.md) es obligatoria antes de habilitar operaciones destructivas.
- Para MySQL, MariaDB y PostgreSQL se evaluarán mecanismos oficiales de volcado y restauración por separado, con inventario de objetos cubiertos, versiones compatibles, requisitos de permisos y estrategia de distribución nativa por sistema operativo.
- Un proveedor no se marca como listo hasta pasar una prueba de exportación y restauración con estructura, datos y objetos representativos. Si no puede garantizar el punto requerido, el core bloquea la acción destructiva.
- La política de respaldo por clase de cambio y la retención manual del MVP se definen en [010 — Respaldos y recuperación](010-respaldos-y-recuperacion.md). El margen de espacio libre se fija antes de implementar operaciones destructivas.

## Seguridad y límites de datos

- El core recibe rutas de archivos elegidas por la persona y valida el destino final antes de leer, crear o borrar. Las operaciones de SQLite se limitan al archivo seleccionado y a los auxiliares definidos en su spec.
- El frontend no recibe secretos almacenados después de guardar una conexión. Los registros y errores no incluyen contraseñas, claves privadas ni cadenas de conexión con secretos.
- Los artefactos con datos se cifran antes de persistirse. El spec 014 define claves por artefacto, recuperación mediante frase o archivo de clave y paquetes portables; su formato exacto requiere validación antes de activar respaldos protegidos.
- Ningún trabajo largo envía una base completa por IPC como un único mensaje. Resultados, progreso y errores se transfieren por lotes.
- La cancelación distingue entre detener el proceso local y confirmar que el motor remoto no aplicó cambios.

## Verificación y entrega incremental

1. Base de aplicación de escritorio con Windows como primera plataforma, almacén local y contratos IPC neutrales al sistema operativo.
2. Conexiones MySQL, explorador y consultas de lectura.
3. Historial, respaldo MySQL verificado y primera operación de modificación de extremo a extremo.
4. Creación/eliminación, SQL que modifica, importación/exportación y cuadrícula de filas.
5. Adaptadores MariaDB, PostgreSQL y SQLite con las mismas pruebas de aceptación por motor.

El [spec 016](016-plan-sdd-mvp.md) detalla el orden y las puertas de salida. El MVP no se considera completo hasta cubrir las capacidades obligatorias de los specs 000–015 para los cuatro motores y cumplir la matriz de plataforma del [spec 019](019-plataformas.md). Las pruebas de Vitest verifican estados y flujos de interfaz; las pruebas de Rust verifican contratos y transiciones; las pruebas de integración usan bases reales; Playwright valida la interfaz y los recorridos nativos se comprueban en cada sistema que se declare compatible. GitHub Actions debe ejecutar las puertas de calidad de cada target.

## Decisiones pendientes

1. Biblioteca y formato binario de cifrado, pendientes de la validación técnica del spec 014.
2. Margen de espacio libre requerido para respaldos según la política del spec 010.
3. Mecanismo concreto y distribución de herramientas de respaldo para MySQL, MariaDB y PostgreSQL en cada target objetivo.
4. Límite de concurrencia de trabajos de lectura y reglas de bloqueo de modificaciones por proyecto.
5. Versiones mínimas de Windows, macOS, distribuciones Linux y arquitecturas admitidas antes de publicar soporte; ver spec 019.

## Referencias técnicas

- [Tauri 2: comandos, estado y canales](https://v2.tauri.app/develop/calling-rust/).
- [SQLx: motores compatibles, consultas y flujo de filas](https://github.com/transact-rs/sqlx).
- [SQLCipher: exportación entre SQLite legible y cifrado](https://www.zetetic.net/sqlcipher/sqlcipher-api/).
- [PostgreSQL: `pg_dump`](https://www.postgresql.org/docs/current/app-pgdump.html).
- [MySQL: `mysqldump`](https://dev.mysql.com/doc/refman/8.4/en/mysqldump.html).
- [MariaDB: `mariadb-dump`](https://mariadb.com/docs/server/clients-and-utilities/backup-restore-and-import-clients/mariadb-dump).
