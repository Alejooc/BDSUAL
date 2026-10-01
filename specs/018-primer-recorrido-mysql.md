# Spec 018 — Primer recorrido MySQL

Estado: implementación en curso  
Versión: 0.9
Última actualización: 2026-09-29
Depende de: [001 — Conexiones](001-conexiones.md), [002 — Explorador](002-explorador.md), [016 — Plan SDD](016-plan-sdd-mvp.md)

## Objetivo

Construir el primer recorrido de la entrega 1: guardar una conexión MySQL, probarla, abrirla y explorar bases, tablas, vistas y columnas desde Windows. Esta ficha deja visibles los criterios cubiertos y los que aún impiden cerrar la entrega 1.

## Contrato de esta porción

- Transporte implementado: **Directa** a MySQL con TLS opcional según configuración y túnel SSH adicional. El formulario todavía no presenta un selector **Directa / DLL / HTTP Tunnel**. DLL y HTTP Tunnel siguen pendientes de contrato, implementación y validación; no se deben presentar como conectables mientras estén en ese estado. Véase [spec 001](001-conexiones.md).
- `list_connections`, `save_connection`, `test_connection`, `open_connection`, `disconnect_connection` y `remove_connection` gestionan conexiones MySQL.
- `list_databases`, `list_database_objects` y `list_columns` consultan solo una conexión abierta. El explorador solicita cada nivel al expandirlo.
- El árbol incluye nodos independientes de **Procesos**, **Usuarios** y **Variables**, que cargan al expandirse y se pueden actualizar por separado. Los comandos consultan la lista visible de procesos sin texto SQL, cuentas visibles sin hashes y variables globales con redacción por nombre. La integración MySQL Community 8.4.11 verificó la consulta y el contrato de proceso/cuenta/variable con `root`; la UI completa, la cuenta de permisos mínimos y el comportamiento sin privilegios siguen pendientes.
- El core valida campos, conserva un identificador estable y persiste únicamente metadatos en el SQLite local (migración 3). La contraseña MySQL y los secretos SSH se guardan en un paquete versionado dentro del Administrador de credenciales de Windows; nunca regresan al frontend tras guardarlos. Actualizar los datos SSH conserva la contraseña MySQL existente.
- Editar una conexión activa la desconecta. Quitarla elimina la configuración y su credencial sin ejecutar instrucciones de borrado en MySQL.
- TLS con verificación de identidad está activado por defecto; se puede desactivar de forma explícita. Se usan las autoridades de Windows o un certificado CA propio; se pueden indicar certificado y clave PEM de cliente sin frase de paso.
- SSH admite contraseña, clave privada con frase de paso y agente OpenSSH de Windows. La clave de host se valida contra el registro local: una clave desconocida requiere comparar y escribir la huella antes de aprobarla; una clave cambiada bloquea la conexión. Con SSH+TLS, el socket usa loopback mientras TLS conserva la validación del nombre original MySQL (`VerifyIdentity`).
- Los errores de validación, credenciales, tiempo de espera, almacén de credenciales y metadatos tienen códigos IPC estables. No incluyen la contraseña ni una URL con credenciales.

## Pruebas necesarias

1. Campos inválidos se rechazan antes de intentar la red.
2. Una prueba exitosa no guarda la conexión ni deja un pool abierto.
3. Guardar, reiniciar y abrir recupera la contraseña desde Windows; cambiar solo el nombre la conserva.
4. Desconectar o quitar impide seguir explorando el pool anterior.
5. El explorador lista solo objetos visibles y diferencia vacío de error en cada nivel. Indica claves primarias y valores predeterminados cuando están disponibles.
6. TLS requerido falla ante certificado inválido y no continúa sin cifrado.
7. El SQLite interno no contiene contraseñas.

## Pendiente para cerrar la entrega 1

- Probar túnel SSH con contraseña, archivo de clave y agente OpenSSH de Windows contra un servidor SSH real, incluida clave desconocida y cambio de clave.
- Resolver el desbloqueo seguro de claves TLS de cliente protegidas con frase de paso.
- Ejecutar la matriz de pruebas contra un servidor MySQL desechable con cuentas, permisos y certificados de prueba. Sin servidor disponible, la compilación y las pruebas de validación no acreditan los criterios que dependen de MySQL real.

## Entrega 2 — consulta SQL (implementación parcial)

- `execute_read_query` se registra como comando Tauri. Rechaza sentencias que no sean un único `SELECT`, limita la consulta a 64 KiB, ejecuta dentro de una transacción MySQL `READ ONLY`, impone 15 segundos y devuelve hasta 200 filas/1 MiB por lote.
- La interfaz abre Monaco localmente (sin CDN), fija conexión/base de destino y presenta errores sin guardar consultas ni resultados. El editor se carga de forma diferida.
- El core obtiene los metadatos del statement preparado antes de leer filas; así conserva los nombres de columna aunque una consulta válida devuelva cero filas. Las consultas y páginas se comprobaron contra MySQL real.
- La interfaz permite cargar tramos adicionales de 200 filas mediante `LIMIT/OFFSET`; muestra que la consulta se repite y que escrituras concurrentes pueden desplazar filas. **Cancelar** solicita `KILL QUERY` desde otra sesión; el estado confirmado se muestra al recibir MySQL el error de interrupción de esa consulta. Pendiente: comportamiento ante desconexión/cierre mientras una consulta está activa. No se debe marcar la entrega 2 como completa hasta resolverlo.

### Prueba real reproducible MySQL 8.4

- Se añadió `connections::tests::mysql_service_connection_and_read_query_work_against_a_real_server`, que usa `connect`, `server_info`, el explorador y el mismo ejecutor SQL de DBSUAL. Crea un esquema aleatorio, verifica objetos/columnas, metadatos de resultados vacíos, paginación ordenada de 205 filas y cancelación de `SELECT SLEEP`; exporta CSV con separadores, comillas, salto de línea, NULL y bytes binarios, comprueba que no se sobrescribe un archivo y elimina los temporales de prueba.
- Ejecutada el 2026-09-28 contra MySQL Community Server 8.4.11 en un contenedor desechable (`mysql:8.4`, imagen `sha256:0744ee5ef89ce6ccfa13de3e579fe6b9e27f93dd70da9c06d2c908b1b193fb8d`): pasó con exportación CSV, lectura en dos tramos (200 + 5 filas) y cancelación confirmada. La prueba usó `tls_mode=disabled`; no acredita TLS, permisos mínimos, SSH ni respaldos.
- Revalidada el 2026-09-30 contra MySQL Community 8.0.46 en un servidor temporal WSL. Pasaron metadatos seguros, lectura SQL con paginación/cancelación, orden/filtro de cuadrícula, creación de base mediante historial y edición/compensación de fila, incluyendo conflictos externos. La prueba SQL de lectura/cancelación falló una vez al comprobar la limpieza de una restauración cancelada y pasó al reintentarla aislada; se amplió la espera de limpieza y debe repetirse en la matriz. Se ejecutó como `root`, sin TLS; no prueba permisos mínimos, TLS, SSH ni IPC nativo de escritura.
- La prueba ignorada `mysql_server_metadata_queries_return_only_safe_process_fields` pasó el 2026-09-29 contra MySQL Community 8.4.11 en Docker. Comprobó lectura de procesos sin campo de texto de consulta, listado de cuentas visibles sin hashes y variables globales. Se ejecutó como `root`; no acredita qué subconjunto ve una cuenta de mínimos privilegios ni el recorrido IPC de la ventana nativa.
- Extendida el 2026-09-29: el fixture crea tablas con PK compuesta, índice compuesto y FK compuesta; `table_structure` devolvió el orden de columnas locales y referenciadas, y la prueba pasó contra MySQL Community Server 8.4 en contenedor desechable. La interfaz ya presenta columnas, índices y restricciones desde **Ver estructura**, pero no se ha probado aún el recorrido IPC de esa ficha en la app nativa.
- Para repetirla en una instancia desechable: define `DBSUAL_MYSQL_TEST_HOST`, `DBSUAL_MYSQL_TEST_PORT`, `DBSUAL_MYSQL_TEST_USER` y `DBSUAL_MYSQL_TEST_PASSWORD`, y ejecuta `cargo test --manifest-path src-tauri/Cargo.toml --locked mysql_service_connection_and_read_query_work_against_a_real_server -- --ignored --nocapture`. La prueba está ignorada en la suite habitual para que no requiera ni toque una base personal.

## Evidencia actual

- La aplicación nativa abrió en Windows y migró el SQLite local a la versión 2 sin perder preferencias.
- El 2026-09-29 pasaron `npm run build`, `npm run test:unit` (3 pruebas), `npm run test:e2e -- --workers=1` (9 pruebas), `cargo fmt --check` y `cargo test --locked` (49 pruebas; 3 integraciones ignoradas por defecto). Las tres integraciones MySQL ignoradas se ejecutaron contra MySQL Community 8.4.11 en un contenedor desechable y pasaron: metadatos del servidor, conexión/lectura/captura/restauración y rechazo de objetos programables antes de crear destino, y flujo protegido de creación de base con conflicto externo.
- El formulario valida campos obligatorios y, fuera de Tauri, informa que el core no está disponible sin fingir una conexión.
- Las conexiones nuevas proponen TLS con verificación de identidad; el explorador obtiene disponibilidad de clave primaria y default del tipo de objeto confirmado en MySQL.
- La integración MySQL 8.4.11 del 2026-09-29 pasó con lectura paginada y estructura compuesta. La ejecución anterior de limpieza SQLite temporal tuvo un bloqueo transitorio de Windows y pasó al repetirla sola y en la suite final. La revisión anterior de dependencias informó cero vulnerabilidades; Playwright usa el preview web y no cubre IPC nativo.
- Repetición focalizada el 2026-09-30 contra MySQL Community 8.4.11: las pruebas de metadatos seguros, conexión/lectura y orden/filtro paginado pasaron. La prueba de conexión/lectura descubrió una carrera en la limpieza del destino provisional al cancelar una restauración; el core ahora reintenta el `DROP DATABASE IF EXISTS` asíncrono ante fallos transitorios, y la misma integración pasó al repetirla. La creación de base mediante historial también volvió a pasar (ver spec 003). Todas usaron `root`; permisos mínimos y comandos IPC en la ventana Tauri siguen pendientes.
- Las pruebas E2E usan el preview web: no validan IPC nativo, túnel SSH, TLS ni consultas contra un motor. La integración se ejecutó en un contenedor desechable, que se retiró al terminar. La interfaz de selección de destino del diálogo nativo todavía requiere recorrido manual en Windows.

### Evidencia adicional — catálogo para respaldos MySQL

- El comando `inspect_mysql_backup_catalog` informa versión del servidor, charset/collation, objetos visibles, DDL y si todas las tablas visibles usan InnoDB. No asume que una lista vacía significa cobertura completa.
- El core puede leer `SHOW CREATE` de tablas, vistas, procedimientos/funciones, triggers y eventos. Se comprobó junto con el inventario contra MySQL Community Server 8.4.11 en un contenedor desechable el 2026-09-29; la prueba crea los objetos, verifica su DDL y confirma que una tabla MyISAM invalida la condición de tablas transaccionales.
- La capa interna transmite filas de tablas base como NDJSON con memoria acotada; valores en hex del protocolo de texto y `NULL` separado. La prueba real incluye binario con `00/FF`, texto literal `\\N`, tabla vacía, round-trip del stream completo mediante el cifrado autenticado y rechazo de MyISAM sin artefacto publicado. Las definiciones del stream incluyen las vistas y sus dependencias estructuradas.
- La captura cifrada toma `LOCK INSTANCE FOR BACKUP` (límite de espera de 10 s) y una transacción InnoDB `REPEATABLE READ` con snapshot consistente de solo lectura en la misma sesión. Pasó en MySQL 8.4.11 con root; una sesión concurrente intentó `ALTER TABLE` y MySQL la rechazó mientras el bloqueo estaba activo. Aún falta validar el error por falta de `BACKUP_ADMIN` con permisos mínimos.
- La captura de historial permite puntos `captured` con cobertura parcial `visible_tables_views_triggers`. La integración `mysql_service_connection_and_read_query_work_against_a_real_server` pasó el 2026-09-29 contra MySQL Community 8.4.11 en contenedor desechable (`mysql:8.4`): restauró tres tablas/seis filas, dos vistas dependientes y un trigger local simple; el conteo de auditoría no duplicó filas al restaurar y el trigger funcionó en una inserción posterior. También validó claves circulares, columna `INVISIBLE`, hash/footer alterados, `DEFINER` no disponible antes de crear destino y limpieza al cancelar. Las pruebas unitarias rechazan cuerpos compuestos, referencias a otros esquemas y órdenes explícitos no soportados. Rutinas, funciones y eventos siguen fuera de cobertura. MySQL 8.0, MariaDB y permisos mínimos siguen pendientes; la captura parcial no habilita operaciones destructivas.
- El formato del manifiesto conserva `DEFINER` para todos los objetos inventariados y además `SQL_MODE`, juego de caracteres, collation de conexión y collation de base para rutinas, triggers y eventos. El restaurador mantiene la puerta cerrada para esos cuatro tipos antes de crear el destino. La integración comprobó ese metadata y el rechazo previo contra MySQL Community 8.4.11 el 2026-09-29. La cobertura recuperable continúa siendo tablas y vistas locales.
