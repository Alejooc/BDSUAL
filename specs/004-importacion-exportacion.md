# Spec 004 — Importación y exportación SQL y CSV

Estado: implementación parcial — exportación CSV MySQL, preview y core de importación protegida (sin habilitar hasta integración real)
Versión: 0.7
Última actualización: 2026-09-30
Depende de: [001 — Conexiones](001-conexiones.md), [002 — Explorador](002-explorador.md)

Toda importación se prepara y aplica mediante el flujo de [006 — Control de cambios](006-control-de-cambios.md). La exportación es una lectura y puede ejecutarse sin preparar una revisión.

## Implementación presente

- Desde una tabla del explorador MySQL se pueden exportar todas las filas accesibles en CSV UTF-8 por flujo, con coma, punto y coma o tabulación, y marcador NULL configurable.
- Las celdas binarias se representan como hexadecimal `0x…`. Encabezados y filas se escapan con el formato CSV. Para distinguir NULL del mismo texto, una celda no nula que coincida con el marcador configurado o empiece con `\` recibe una barra invertida inicial adicional; la importación deberá retirar esa barra antes de aplicar el marcador NULL. El core escribe en un temporal hermano, sincroniza el contenido y solo publica el archivo completo; nunca reemplaza una ruta existente.
- Prueba real MySQL 8.4.11: verifica encabezados, Unicode, coma, comillas, salto de línea, NULL, bytes binarios, colisión con el marcador NULL y rechazo de destino existente.
- Se añadió un prototipo de parser Rust aislado para CSV UTF-8: límites de 5 MiB, 50 000 filas y 512 columnas; valida encabezados no vacíos/únicos, anchura uniforme y decodifica el escape de marcador NULL que produce el exportador. No está conectado a comandos ni a la interfaz, por lo que no constituye una importación disponible.
- El core Rust registra `prepare_mysql_csv_import` y `prepare_mysql_csv_import_revert`, persiste el plan exclusivamente como artefacto cifrado y aplica/revierte filas mediante una transacción MySQL. Revalida identidad del servidor y esquema, bloquea triggers/FK entrantes y conflictos, limita el lote a 10 000 filas/2 MiB de plan y verifica cada fila antes de confirmar.
- La integración `mysql_csv_batch_applies_atomically_and_compensation_blocks_external_changes` pasó en Windows contra MySQL Community 8.0.46 desechable. Verificó una colisión después de preparar sin inserciones parciales, aplicación del lote, bloqueo de reversión tras una edición externa y compensación al restaurar los valores. La vista previa E2E valida CSV/NULL y confirma que no se invoca IPC de mutación. Un test con `MockRuntime` también despacha el comando Tauri de preparación y verifica por IPC el error estable `CSV_IMPORT_INVALID` para separador inválido, antes de abrir almacenamiento; no ejecuta escrituras en MySQL. El recorrido de escritura desde la ventana Tauri nativa sigue pendiente y la acción permanece deshabilitada. Revalidación del 2026-09-30: `cargo test --locked` pasó (81; 14 ignoradas), `cargo fmt --check` pasó, Vitest pasó (3) y Playwright pasó (24/24); estas suites no cierran el recorrido nativo.
- Pendiente: cubrir límites/permisos y efectos de SQL de las versiones soportadas, ejecutar la integración real en MySQL, invocar comandos por IPC nativo y verificar recuperación ante interrupciones. También faltan exportación/importación SQL de base completa, progreso y cancelación.

## Objetivo

Exportar e importar una base de datos completa mediante SQL, con estructura y datos, y exportar e importar filas de una tabla mediante CSV. Cada archivo SQL usa el dialecto de su motor; no se promete restauración directa entre MySQL, MariaDB, PostgreSQL y SQLite.

## Exportación SQL de una base de datos

1. La persona elige una base de datos, una ruta de destino y **Exportar SQL**.
2. La aplicación muestra el alcance de la exportación y comprueba permisos, espacio y acceso al destino antes de comenzar cuando sea posible.
3. El archivo contiene instrucciones suficientes para reconstruir la estructura y los datos que la aplicación declare compatibles para ese motor. Debe incluir el orden correcto de dependencias entre objetos.
4. El progreso distingue preparación, lectura y escritura. Si una categoría de objetos no puede exportarse por permisos o por falta de soporte, la operación no se anuncia como «base completa»; se informa exactamente qué faltó.
5. El resultado se escribe primero en un archivo temporal. Solo se presenta como exportación completa cuando el archivo final está íntegro y se ha cerrado correctamente.
6. Las credenciales, secretos y datos internos de DBSUAL no se incluyen en el archivo.

Para llamar «completa» a una exportación se deben cubrir, cuando existan en el motor, tablas y datos, vistas, índices, restricciones, disparadores, rutinas y demás objetos propios de la base. El inventario exacto y las opciones de consistencia se verificarán por motor antes de implementar cada exportador.

## Importación SQL de una base de datos

1. La persona selecciona un archivo SQL, el motor de destino y la conexión.
2. La aplicación muestra una vista previa del destino y advierte que el archivo puede contener operaciones destructivas.
3. Se comprueba que el archivo corresponde al motor declarado y se prepara una revisión con un resumen de operaciones identificables. La vista previa no promete detectar todos los efectos de SQL arbitrario.
4. Tras confirmar la revisión, **Aplicar a la base** ejecuta la importación con progreso y registro de errores por etapa. Si falla, se informa qué se aplicó y qué no se puede determinar con certeza.
5. La base de datos de destino se actualiza en el explorador solo después de comprobar su estado.

La importación de SQL arbitrario puede contener instrucciones que no admiten deshacer. Su relación con el flujo de preparación y recuperación se define en `006-control-de-cambios.md`.

## Exportación CSV por tabla

- Desde **Más opciones > Exportar** en la fila de una tabla, la persona elige **CSV** entre los formatos compatibles. Luego indica la ruta de destino y las opciones CSV antes de iniciar la exportación. El menú no inicia una exportación ni sobrescribe una ruta sin confirmación/reglas de destino claras.
- Una exportación corresponde a una tabla y contiene una fila de encabezados con nombres de columnas y las filas de datos accesibles.
- El archivo se genera en UTF-8. La interfaz permite elegir separador y representación de valores nulos antes de comenzar.
- Los valores se escapan según el formato CSV elegido. La exportación no incluye definición de tabla, tipos, índices ni relaciones.
- Se informa el número de filas exportadas y cualquier límite o interrupción. Un resultado parcial no se presenta como completo.

## Importación CSV por tabla

- La persona selecciona un archivo CSV y una tabla de destino existente.
- Antes de ejecutar, ve una muestra de filas y asigna columnas del archivo a columnas de la tabla. Puede ajustar separador, codificación y representación de valores nulos.
- La aplicación valida encabezados, tipos y restricciones que pueda detectar, pero el motor conserva la decisión final sobre cada fila.
- La importación se prepara como revisión. Confirmarla no inserta filas; **Aplicar a la base** inicia la operación.
- Se muestra el número de filas aceptadas y rechazadas y los errores correspondientes. Por defecto la importación es **todo o nada por tabla** si el motor y la tabla admiten una transacción real. Si no, se aplica la política del [spec 015](015-conflictos-y-aplicacion.md): respaldo completo, resultado parcial visible o bloqueo cuando no pueda medirse. Nunca se ocultan filas fallidas.
- La importación respeta los permisos y restricciones de la tabla de destino.
- La primera importación protegida CSV se limita a añadir filas MySQL: tabla InnoDB con PK, solo tipos escalares ya admitidos por la inserción protegida, sin triggers ni FK entrantes. Rechaza claves duplicadas en el archivo y ya existentes antes de crear la revisión. El lote se aplica como una sola transacción InnoDB. La reversión es otra revisión que solo elimina filas que aún coincidan en todas sus columnas con los valores insertados. No se habilitan reemplazo, actualización, borrado de filas existentes ni SQL arbitrario; para sus efectos sigue rigiendo el respaldo completo del spec 010.

Contrato IPC: `prepare_mysql_csv_import` recibe `{ connectionId, databaseName, tableName, csvText, delimiter, nullMarker }`, donde `delimiter` es `comma | semicolon | tab`; responde `{ revision, headers, rowCount, sampleRows }`. `sampleRows` conserva `null` real como JSON `null` y se limita a cinco filas. `prepare_mysql_csv_import_revert` recibe `{ revisionId }` y devuelve el mismo resumen con la revisión compensatoria. Ambos comandos preparan solamente; `apply_mysql_row_update` despacha estas revisiones confirmadas por historial y aplica todo el lote en una transacción. Errores usan códigos estables y no contienen filas, CSV ni secretos. Ningún valor de CSV se persiste en SQLite ni en logs; el plan JSON completo solo se conserva como artefacto cifrado.

## Criterios de aceptación iniciales

1. Dada una base MySQL con estructura y datos representativos, exportar SQL y restaurar en una base vacía del mismo motor reproduce los objetos y filas cubiertos por el alcance declarado.
2. Si faltan permisos para exportar un objeto requerido, la aplicación identifica el objeto o categoría y no declara completo el archivo.
3. Dada una tabla con comas, saltos de línea, comillas y valores nulos, la exportación CSV y su posterior importación conservan los valores según la configuración elegida.
4. Dado un archivo CSV con columnas incompatibles, la vista previa advierte el problema antes de importar y la ejecución informa los errores reales del motor.
5. Dada una exportación interrumpida, el archivo parcial no se publica como resultado completo.
6. Dada una importación SQL que falla después de aplicar alguna instrucción, la interfaz no afirma que todo se revirtió; muestra el estado conocido y permite volver a inspeccionar la base.

## Decisiones pendientes

1. Si la importación SQL puede apuntar a una base existente o solo crear/restaurar una base nueva en la primera entrega.
2. Si una versión posterior ofrecerá continuar con filas CSV válidas tras rechazos; el MVP usa el criterio del spec 015.
3. Si se ofrecerá exportar varias tablas CSV en una sola operación.
4. Inventario y mecanismo de exportación SQL completo para cada motor, con pruebas de restauración y consistencia.
5. Formatos adicionales del menú **Exportar** y opciones específicas de cada uno; solo se mostrarán como habilitados cuando el motor y la capacidad estén implementados.

### Evidencia de comandos IPC (2026-09-30)

El test ignorado `mysql_csv_import_and_compensation_round_trip_through_mock_tauri_ipc` ejecuta los comandos Tauri de producción mediante `MockRuntime` contra MySQL Community 8.4.11 desechable. Comprobó: preparar deja la tabla vacía; confirmar la revisión tampoco escribe; aplicar importa dos filas; preparar y confirmar la compensación mantiene las filas; aplicar la compensación las elimina. El test usa un runtime Tokio multi hilo para que la espera síncrona de `get_ipc_response` no detenga SQLx. Esta evidencia valida el contrato IPC y el ciclo con un servidor real, pero no reemplaza el recorrido en ventana Tauri nativa ni habilita la acción de la interfaz.
