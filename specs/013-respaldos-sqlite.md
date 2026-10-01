# Spec 013 — Proveedor de respaldos SQLite

Estado: borrador para revisión  
Versión: 0.3  
Última actualización: 2026-09-28  
Depende de: [003 — Crear y eliminar bases de datos](003-operaciones-bases-de-datos.md), [010 — Respaldos y recuperación](010-respaldos-y-recuperacion.md)

## Objetivo

Crear y restaurar copias consistentes de bases SQLite almacenadas en archivos locales de Windows, macOS o Linux. La copia protegida debe incluir transacciones confirmadas que todavía estén en el WAL y no depender de copiar directamente el archivo principal mientras la base está abierta.

## Alcance e identidad

- Una conexión SQLite apunta a un archivo de base de datos. El proveedor identifica la ruta absoluta y su identidad de archivo antes de crear o restaurar una copia.
- Cada archivo adjunto mediante `ATTACH` es otra base de datos y no forma parte implícita de la copia de `main`. La interfaz enumera las bases adjuntas conocidas y explica que necesitan puntos separados.
- Los archivos externos usados por extensiones o tablas virtuales no quedan incluidos solo por copiar el archivo SQLite. El proveedor detecta dependencias conocidas y muestra el límite de recuperación.
- Las bases `:memory:` quedan fuera de este proveedor en el primer MVP.
- El artefacto registra versión de la biblioteca SQLite, tamaño, hash, esquema y fecha de la instantánea.

## Decisión técnica para el respaldo cifrado

- **Candidato del MVP:** usar SQLCipher Community Edition para abrir la base SQLite original sin clave, adjuntar un destino nuevo cifrado y ejecutar `sqlcipher_export()` directamente hacia ese destino. El archivo intermedio gestionado por DBSUAL ya debe estar cifrado; no se crea antes otra copia SQLite legible en disco.
- La Online Backup API de SQLCipher sirve para copias entre bases del mismo tipo, pero **rechaza una fuente sin cifrar y un destino cifrado**. Por tanto, no se empleará para esa conversión. Tampoco se copiará el archivo principal con operaciones ordinarias mientras la base esté activa.
- La base original puede estar en WAL. La exportación debe leer una vista coherente que incluya transacciones confirmadas aún presentes en el WAL. La consistencia bajo escrituras concurrentes, el tratamiento de bloqueos y la posibilidad de cancelar deben comprobarse en cada sistema operativo que use el proveedor antes de habilitarlo. La documentación de `sqlcipher_export()` demuestra la conversión, pero no sustituye esa prueba.
- `sqlcipher_export()` no transfiere `user_version` ni `auto_vacuum`. El proveedor registra sus valores y los establece en el destino cuando proceda; también verifica las propiedades y objetos de la base que la aplicación promete preservar.
- El artefacto declara formato y versión de SQLCipher. Su clave de datos se envuelve y recupera según el [spec 014](014-claves-y-artefactos-cifrados.md); el manifiesto portable acompaña siempre al archivo SQLCipher. No se imprime la clave en comandos, mensajes IPC ni registros. La integración con SQLx y el empaquetado de SQLCipher se validan por target antes de elegir el enlace definitivo.

## Creación de la copia

1. Se fija la identidad del archivo fuente y se abre con el proveedor elegido. Se comprueban permisos, espacio, estado del WAL y valores de metadatos que deben conservarse.
2. Se crea un destino cifrado con nombre de trabajo no publicado y se ejecuta la exportación. Un error, cancelación o cierre inesperado deja el trabajo pendiente de reconciliación; ningún archivo parcial se registra como punto válido.
3. Se cierra y vuelve a abrir el destino con su clave. Se ejecutan `PRAGMA integrity_check`, `PRAGMA foreign_key_check` y `PRAGMA cipher_integrity_check`, y se comprueban `user_version`, `auto_vacuum` e inventario de tablas, vistas, disparadores e índices. Un fallo impide usarlo como punto protegido.
4. Se calcula el hash del archivo cifrado final y se registra motor, formato, fecha, tamaño e identidad de la base fuente. Solo después se publica el punto de recuperación y se habilita la operación destructiva.

El archivo `-wal` forma parte del estado de una base activa. DBSUAL no lo elimina ni lo ignora manualmente para preparar el respaldo.

## Restauración

1. Por defecto, la persona elige una ruta nueva. DBSUAL comprueba que no existe un archivo que pueda sobrescribirse.
2. Abre el artefacto con SQLCipher y exporta directamente a un nuevo archivo SQLite legible en la ruta elegida. El archivo de salida se considera incompleto hasta repetir las comprobaciones de integridad e inventario; tras una interrupción se reconcilia antes de ofrecerlo como base recuperada.
3. Si falla, retira el archivo de salida incompleto y deja intacta la base original; el evento de historial registra el fallo. Tras una caída, la reconciliación identifica esa salida incompleta antes de permitir reutilizar la ruta.
4. Reemplazar un archivo existente requiere una revisión separada, copia del estado actual, confirmación de la ruta exacta y cierre seguro de todas las conexiones de DBSUAL a ese archivo.
5. Si otra aplicación mantiene abierto el archivo o hay un WAL/diario cuyo estado no puede consolidarse con seguridad, se bloquea el reemplazo. DBSUAL no borra manualmente el WAL para forzarlo.
6. Después del reemplazo, se abre la base desde su ruta final, se verifica de nuevo y se registra el resultado. Si la sustitución queda incierta, no se presenta como restauración exitosa.

## Exportación SQL

- **Exportar SQL** genera un archivo legible que reconstruye esquema y datos del archivo SQLite. Se evalúa el comportamiento de `.dump` de la CLI oficial como referencia funcional.
- El SQL exportado y el respaldo protegido del historial son productos distintos. El respaldo protegido conserva una imagen SQLite verificable; la exportación SQL se puede importar con el flujo del spec 004.
- Las tablas virtuales y extensiones se prueban en una restauración representativa antes de declararlas cubiertas por una exportación completa.

## Seguridad y límites por plataforma

- Se valida la ruta final para impedir que un cambio de enlace, unidad o nombre entre vista previa y aplicación redirija una eliminación o reemplazo a otro archivo.
- Los respaldos gestionados por DBSUAL se guardan cifrados. Una exportación SQL legible solicitada por la persona es un producto distinto y se guarda únicamente en la ruta que ella elija.
- La creación del respaldo no usa un archivo SQLite temporal sin cifrar. Si la implementación finalmente necesita uno, esta decisión se reabre y el proveedor no se habilita hasta revisar el riesgo y probar su limpieza después de fallos.
- Un archivo de base muy grande puede necesitar espacio temporal adicional. Si la estimación y el margen disponible no bastan, no se inicia la operación protegida.

## Criterios de aceptación

1. Con una base abierta en modo WAL y transacciones confirmadas aún no consolidadas en el archivo principal, el respaldo restaurado incluye esos datos.
2. Una copia creada mientras otra conexión escribe termina como instantánea coherente o falla de forma explícita; nunca se marca completo un archivo parcial.
3. `integrity_check` y `foreign_key_check` se ejecutan sobre el resultado; un error bloquea su uso como punto de recuperación.
4. Restaurar en una ruta nueva no sobrescribe el archivo original y produce una base que puede abrirse y consultarse.
5. Si otra aplicación bloquea el archivo original, reemplazarlo se detiene sin borrar el archivo ni el WAL.
6. Si faltan espacio o clave de cifrado, la operación destructiva que necesita el respaldo no comienza.
7. Un archivo SQLite adjunto o un recurso externo de una extensión no se presenta falsamente como incluido en la copia de `main`.
8. La copia creada desde una base SQLite legible no contiene encabezado ni páginas legibles por SQLite estándar; SQLCipher la abre con la clave correcta y la rechaza con una clave incorrecta.
9. Una interrupción en cada fase de creación deja, como máximo, un artefacto cifrado incompleto que no aparece como punto válido; la recuperación del trabajo lo identifica y limpia o conserva para diagnóstico según la política local.
10. Una prueba con escrituras concurrentes en WAL comprueba una instantánea coherente, sin mezcla de estados entre tablas relacionadas; también mide bloqueo y cancelación con una base grande.
11. `user_version`, `auto_vacuum`, disparadores, índices y objetos admitidos coinciden tras respaldo y restauración; las excepciones se muestran antes de permitir la operación destructiva.

## Decisiones pendientes

1. Prueba técnica de `sqlcipher_export()` por cada target declarado, con WAL, escrituras concurrentes, cancelación, caída y restauración; hasta superarla, el proveedor SQLite no se considera listo para operaciones destructivas en ese target.
2. Margen de espacio para copia y restauración según tamaño de base y método elegido.
3. Compatibilidad de extensiones y tablas virtuales en la exportación SQL completa.
4. Versión, enlace, distribución y avisos de licencia de SQLCipher Community Edition en los instaladores de cada target.

## Referencias técnicas

- [SQLite: Online Backup API](https://www.sqlite.org/backup.html).
- [SQLite: archivos WAL](https://www.sqlite.org/wal.html).
- [SQLite: `integrity_check` y `foreign_key_check`](https://www.sqlite.org/pragma.html).
- [SQLite: exportación `.dump`](https://www.sqlite.org/cli.html).
- [SQLCipher: `ATTACH` y `sqlcipher_export()`](https://www.zetetic.net/sqlcipher/sqlcipher-api/).
- [SQLCipher: pruebas oficiales de la API de respaldo, incluida la restricción entre texto claro y cifrado](https://github.com/sqlcipher/sqlcipher/blob/master/test/sqlcipher-backup.test).
- [SQLCipher: comportamiento de `sqlcipher_export()` con una base activa](https://discuss.zetetic.net/t/locking-and-sqlcipher-export-with-a-busy-database/5716).

## Avance SQLite — 2026-09-30

La conexión de archivo, la enumeración de `main`, tablas/vistas y metadatos de columnas, y la lectura paginada de tablas están implementadas en el adaptador SQLite. Los archivos se canonicalizan y se abren en modo de solo lectura; los archivos faltantes no se crean. La cuadrícula valida identificadores con el catálogo, enlaza valores de filtro, conserva `NULL`, representa blobs como hexadecimal y respeta límites de 200 filas y aproximadamente 1 MiB por página, con filas completas.

La eliminación del archivo, escritura SQL, importación, exportación SQL, historial de recuperación y respaldos cifrados SQLite no están habilitados. Los adjuntos solo se enumeran; la exploración se limita a `main`. El criterio de copia SQLCipher con WAL, concurrencia y restauración del spec sigue pendiente y bloquea cualquier operación destructiva.

Verificación disponible: pruebas Rust con un archivo SQLite temporal verifican acceso de solo lectura, catálogo, metadatos, valores `NULL`, rechazo de rutas inexistentes y rechazo de bases distintas de `main`. Aún falta un recorrido de la ventana Tauri nativa en Windows.

La lectura de la cuadrícula ahora aplica por defecto el orden de la clave primaria declarada (incluyendo claves compuestas) cuando no se eligió otro orden. Esto hace reproducibles las páginas por offset en tablas con clave; las tablas sin clave primaria conservan el orden que entrega SQLite. No cambia el alcance de solo lectura ni habilita escritura. Una prueba con claves compuestas insertadas fuera de secuencia comprueba el orden por defecto; falta verificar el recorrido nativo.

Spike aislado añadido en `spikes/sqlcipher-wal`: una prueba prepara un archivo temporal SQLite en modo WAL, confirma filas relacionadas sin checkpoint automático y exporta con `sqlcipher_export()` a SQLCipher; comprueba cifrado, apertura con clave válida, rechazo de clave incorrecta, `integrity_check` y `foreign_key_check`. Usa `rusqlite` 0.32.1 con SQLCipher vendorizado; no está integrado al binario DBSUAL ni habilita escrituras o eliminaciones del producto. **Evidencia comprobada solo en Linux:** `cargo test --locked` pasó en `rust:1.98.0-bookworm` el 2026-09-30 (1 prueba), incluyendo una verificación de que el archivo fuente mantenía frames WAL después del commit. En Windows 11 la compilación se detuvo antes de ejecutar en `openssl-sys` porque el Perl disponible en Git for Windows carece de `Locale::Maketext::Simple`; por tanto no se cuenta como evidencia Windows ni cierra el criterio de aceptación. Comando: `cargo test --manifest-path spikes/sqlcipher-wal/Cargo.toml --locked`.
