# Spec 012 — Proveedor de respaldos PostgreSQL

Estado: diseño; proveedor de respaldo no implementado
Versión: 0.2
Última actualización: 2026-09-29
Depende de: [004 — Importación y exportación](004-importacion-exportacion.md), [010 — Respaldos y recuperación](010-respaldos-y-recuperacion.md)

## Objetivo

Definir una copia completa y restaurable de una base PostgreSQL en Windows, macOS y Linux, con un proveedor independiente de MySQL y MariaDB. La copia protegida sirve como punto de recuperación antes de una operación destructiva; la exportación SQL del usuario sigue el spec 004. La distribución de herramientas y los recorridos nativos se prueban por sistema según el spec 019.

## Detección y compatibilidad

- El core identifica la versión mayor del servidor al abrir la conexión y selecciona una versión compatible de las herramientas de copia y restauración.
- `pg_dump` no se usa contra un servidor más nuevo que su propia versión mayor. La restauración protegida se prueba en una versión de destino admitida; no se presupone que un volcado se restaure en un servidor más antiguo.
- El artefacto registra versiones de servidor, `pg_dump`, formato y opciones relevantes. Una versión fuera de la matriz puede seguir disponible para exploración y consultas, pero no para acciones que exijan un respaldo protegido hasta validar su proveedor.
- Matriz provisional de pruebas: PostgreSQL 16, 17 y 18. Se ajustará cuando se confirme la prioridad de versiones.

## Alcance de la copia

- Una copia protegida abarca los esquemas y objetos de **una base**: tablas y datos, vistas y vistas materializadas, secuencias, funciones, índices, restricciones, disparadores, políticas de seguridad por filas, extensiones registradas, publicaciones, definiciones de suscripciones y objetos grandes cuando existan y estén dentro del alcance de `pg_dump`.
- La copia conserva el inventario y las dependencias necesarias para reconstruir esos objetos en un destino compatible. Si faltan permisos sobre objetos requeridos, falla como punto protegido; no se acepta una exportación parcial.
- Roles, tablespaces y otros objetos globales de la instancia no forman parte de `pg_dump` de una base. La interfaz los enumera como dependencias externas y comprueba que el destino de restauración dispone de lo necesario o que se eligió una política explícita de propiedad y permisos.
- Las definiciones de tablas externas y suscripciones no implican que se hayan copiado los datos o el estado almacenados en servidores ajenos. La interfaz distingue ese límite antes de declarar recuperable la base.
- La copia no contiene contraseñas del proyecto ni convierte automáticamente el contenido de extensiones instaladas fuera de la base en parte del artefacto.

## Estrategia de copia

1. El proveedor usa `pg_dump` en formato de archivo apto para inspección y restauración selectiva, como el formato personalizado. El artefacto se cifra durante su escritura y se vincula a la revisión protegida.
2. Comprueba permisos, seguridad por filas y dependencias externas antes de iniciar. Si no puede incluir todas las filas requeridas, bloquea el respaldo protegido; no activa una opción que omita filas protegidas y aun así lo llame completo.
3. Establece un tiempo máximo de espera para bloqueos, informa progreso y permite cancelar antes de aplicar la revisión. Un bloqueo prolongado o error no produce un punto válido.
4. Al terminar, comprueba salida de la herramienta, hash y tabla de contenido del archivo. Compara el inventario con el catálogo accesible y registra diferencias.
5. La copia solo habilita una operación destructiva cuando supera estas comprobaciones. Las pruebas automatizadas de restauración representativa por versión complementan la comprobación de cada archivo.

`pg_dump` permite obtener una exportación coherente sin bloquear las lecturas y escrituras normales, pero pueden existir esperas por cambios de estructura y objetos dependientes externos. La interfaz muestra el estado real de la copia en lugar de prometer ausencia de impacto.

## Restauración

1. Por defecto, la persona elige un nombre nuevo. DBSUAL crea una base vacía desde una conexión administrativa y restaura allí el archivo con `pg_restore` compatible.
2. La operación revisa roles, propietario, privilegios, extensiones y tablespaces necesarios. Si faltan, muestra una decisión explícita antes de restaurar o bloquea cuando no pueda conservar el alcance requerido.
3. La restauración se detiene ante errores y registra qué objetos se reconstruyeron. Cuando sea viable para el tamaño y la versión, usa una transacción única para evitar una base parcialmente restaurada; si no, mantiene el destino nuevo aislado y marcado como incompleto ante fallos.
4. Después compara inventario, estructura y datos verificables con el manifiesto y registra el resultado. La base original permanece intacta durante esta prueba.
5. Reemplazar una base existente es una revisión distinta y exige proteger primero su estado actual según el spec 010. La creación o eliminación de una base no se trata como parte de una transacción SQL ordinaria.

## Exportación SQL

- **Exportar SQL** produce un archivo legible del dialecto PostgreSQL con estructura y datos de una base, de acuerdo con el spec 004.
- El respaldo protegido puede usar un formato de archivo distinto del SQL legible, siempre que el producto pueda inspeccionarlo y restaurarlo mediante el proveedor validado.
- Un archivo SQL entregado por la persona para importar se trata como entrada potencialmente modificadora y se prepara en el historial antes de aplicarlo.

## Seguridad y distribución por plataforma

- DBSUAL identifica la herramienta y su versión; no ejecuta una utilidad encontrada al azar en `PATH` como proveedor protegido.
- La estrategia de distribución permite crear y restaurar copias sin exigir una instalación manual no documentada. Se validan procedencia, licencia y actualizaciones antes de distribuir binarios.
- Contraseñas y claves no aparecen en argumentos visibles de proceso, registros ni errores. Si se requiere un archivo temporal de credenciales, se restringe, se borra al terminar y se recupera tras cierres inesperados.
- TLS y el túnel SSH configurados para la conexión también se aplican a las herramientas de copia y restauración.

## Criterios de aceptación

1. Una base con esquemas, tablas, secuencias, vistas, funciones, disparadores, políticas y datos representativos se copia y restaura en una base nueva de cada versión admitida, con contenido equivalente dentro del alcance declarado.
2. Una cuenta que no puede leer todas las filas por seguridad a nivel de fila no genera un respaldo protegido etiquetado como completo.
3. Si faltan roles, extensiones u otras dependencias del destino, la interfaz las informa antes de restaurar y no declara éxito completo si fallan objetos.
4. Un `pg_dump` más antiguo que el servidor no se usa para un punto protegido.
5. Una copia interrumpida, ilegible o con hash incorrecto no habilita la operación destructiva.
6. Una restauración fallida en una base nueva no modifica la base original y deja el nuevo destino marcado como incompleto.
7. La copia y restauración a través de TLS y túnel SSH no exponen secretos en argumentos visibles ni registros.

## Decisiones pendientes

1. Versiones definitivas de PostgreSQL para la matriz del MVP.
2. Versiones y distribución concreta de `pg_dump`, `pg_restore` y otras utilidades necesarias en cada target (Windows, macOS y Linux).
3. Política de propietario y privilegios cuando la base restaurada vive en otra instancia o carece de roles globales.
4. Tamaño a partir del cual una restauración de transacción única deja de ser viable y qué comprobación alternativa se exige.

## Referencias técnicas

- [PostgreSQL: `pg_dump`](https://www.postgresql.org/docs/current/app-pgdump.html).
- [PostgreSQL: `pg_restore`](https://www.postgresql.org/docs/current/app-pgrestore.html).
- [PostgreSQL: `pg_dumpall` para objetos globales](https://www.postgresql.org/docs/current/app-pg-dumpall.html).
- [PostgreSQL: versiones admitidas](https://www.postgresql.org/support/versioning/).

## Estado de implementación — 2026-09-29

La conexión directa PostgreSQL y el listado de bases accesibles están parcialmente implementados en el spec 001. No existe todavía proveedor con pg_dump/pg_restore, artefacto cifrado PostgreSQL ni restauración probada. Por tanto, este spec permanece sin cumplir y no habilita operaciones destructivas.

La prueba real del adaptador de conexión pasó en un contenedor desechable PostgreSQL 16, pero no ejercita captura/restauración ni el recorrido Tauri. Antes de implementar el proveedor siguen abiertas la distribución confiable de herramientas por plataforma, el manejo de TLS/SSH y credenciales sin argumentos visibles, política de roles/propietarios/extensiones, RLS y la restauración representativa en cada versión. Las versiones 16, 17 y 18 continúan provisionales; la cobertura actual es evidencia del core en Windows, no compatibilidad multiplataforma.
