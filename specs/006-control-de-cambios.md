# Spec 006 — Control de cambios de bases de datos

Estado: implementación parcial; edición localizada de fila MySQL con integración del core verificada
Versión: 1.1
Última actualización: 2026-09-30
Depende de: [003 — Crear y eliminar bases de datos](003-operaciones-bases-de-datos.md), [004 — Importación y exportación](004-importacion-exportacion.md)

Decisión de producto: el historial y los puntos de recuperación son propios de DBSUAL. Git es una integración opcional para compartir cambios de estructura y scripts revisables; no es el registro principal del estado de una base de datos.

Todas las acciones que modifican una base de datos pasan obligatoriamente por **Preparar → Revisar → Confirmar revisión → Aplicar a la base**. No existe un modo de ejecución directa para cambios. Si el historial local no se puede iniciar o guardar, la aplicación no ejecuta la modificación.

## Objetivo

Ofrecer un flujo inspirado en Git para cambios en bases de datos: preparar, revisar, confirmar localmente, aplicar al destino y recuperar un estado anterior cuando exista un respaldo verificable. El alcance incluye estructura, datos y operaciones sobre bases de datos completas.

Una **revisión de DBSUAL** registra una intención de cambio y su contexto; no modifica la base de datos por sí sola. **Aplicar a la base** ejecuta la revisión confirmada sobre el destino. Cada aplicación y restauración agrega un evento inmutable al historial interno.

El historial interno conserva planes de cambio y registros de aplicación sin credenciales. Los archivos SQL o CSV con datos y los respaldos completos se guardan cifrados como artefactos vinculados por hash. La ausencia de un artefacto requerido se muestra de forma visible: una revisión puede seguir siendo consultable aunque no sea posible repetirla o restaurarla.

## Evidencia de implementación

- El almacenamiento local versionado conserva proyectos por conexión y base, resúmenes de revisiones y eventos append-only. Los planes no se guardan como texto en SQLite; se conserva su hash y el identificador del artefacto cifrado.
- La interfaz permite iniciar el historial para una base MySQL/MariaDB existente en una conexión abierta y consultar los resúmenes guardados. El core verifica que la base siga visible, registra versión y distingue MariaDB por su respuesta `VERSION()`. Al quitar una conexión se conserva el proyecto histórico.
- El editor puede guardar una instrucción SQL individual como borrador cifrado en un archivo de artefacto. La interfaz abre el plan, muestra destino y hash y permite confirmarlo localmente. Preparar y confirmar no ejecutan SQL; el estado de recuperación permanece pendiente y bloquea el inicio de aplicación.
- Pruebas Rust verifican identidad estable del proyecto, eventos inmutables, confirmación, bloqueo de aplicación sin recuperación verificada y escritura cifrada de contenido efímero. La prueba de confirmación revisada lee el plan solo después de autenticar el artefacto.
- Al iniciar, las revisiones que quedaron en `applying` se cambian atómicamente a `uncertain` y reciben el evento `interrupted_after_restart`. DBSUAL no reenvía la operación porque el historial local no demuestra si el servidor alcanzó a confirmarla. Una prueba cierra y reabre SQLite, verifica estado/evento y comprueba que repetir la reconciliación no duplique eventos.
- La interfaz traduce estados del historial y muestra un aviso persistente para `uncertain`: comprobar el destino antes de continuar; DBSUAL no repite ni debe reaplicarse esa revisión. Los planes de actualizar/revertir fila se muestran como diferencias de clave, columna, valor anterior y propuesto; contenido inválido no se expone como JSON bruto. La prueba Playwright usa IPC simulado: acredita presentación y ausencia de llamadas de aplicación al abrir, pero no comprueba ni reconcilia el estado remoto.
- **La aplicación general de cambios aún no está habilitada:** los planes SQL generales y las operaciones destructivas continúan bloqueados. Se añadió un recorrido limitado de edición de fila MySQL y revisión compensatoria. La otra excepción es crear una base vacía MySQL, que no reemplaza estado previo.
- La edición de fila solo prepara cambios de celdas no clave en tablas base InnoDB con clave primaria, tipos escalares admitidos y sin triggers. El plan y los metadatos de compensación se guardan cifrados; aplicar exige revisión confirmada, vuelve a comprobar la fila y genera una condición de conflicto si cambió. Tras una aplicación, **Preparar reversión** crea otra revisión con la preimagen; no muta la base directamente ni reescribe el evento original. Playwright verifica la secuencia con IPC simulado. La integración del core y el round-trip por comandos de producción en Tauri MockRuntime pasaron contra MySQL Community 8.4.11 desechable: preparar/confirmar no escriben, aplicar modifica la celda y la revisión compensatoria solo restaura al aplicar. Falta recorrerlo en la ventana Tauri nativa; no se considera un flujo listo para producción.
- Excepción acotada para crear una base MySQL vacía: no hay estado anterior que respaldar. Su revisión se cifra y confirma por separado; al aplicar, se vuelve a verificar `@@server_uuid`, versión y ausencia del nombre. El servicio compartido por los comandos se probó contra MySQL 8.4.11: preparar/confirmar no ejecutan SQL, aplicar crea y registra el resultado, y un nombre ocupado por un cambio externo conserva su base y deja la revisión confirmada. No habilita otros planes SQL ni operaciones destructivas. Falta probar el recorrido IPC y la actualización del explorador en la app nativa.
- Importación CSV MySQL: están implementados los comandos Tauri de preparar/revertir y el despacho de aplicación de revisión confirmada. Se cifra el plan completo como artefacto; SQLite conserva solo referencias/hash/metadatos. La preparación detecta PK duplicadas y existentes, coteja columnas y tipos, limita a 10 000 filas y 2 MiB de plan, y rechaza tablas distintas de InnoDB, triggers y FK entrantes. Aplicación y compensación bloquean las filas y operan como una transacción todo-o-nada, con verificación de valores y conflictos. La integración real en MySQL Community 8.0.46 pasó por servicio y verificó conflicto durante la aplicación, atomicidad y compensación condicionada al estado de las filas. La UI implementa la secuencia pero mantiene la acción deshabilitada hasta recorrerla desde la ventana Tauri nativa en Windows.
- El contrato de importación acotada previsto es un plan cifrado de inserciones solamente, con lista ordenada de filas completas y clave primaria por fila. La preparación comprueba unicidad interna y ausencia de cada clave en el destino. La aplicación vuelve a comprobar esquema, triggers/FK entrantes y claves bajo una única transacción InnoDB; un rechazo revierte el lote entero. La compensación crea otra revisión de eliminación y compara cada fila completa con los valores originales, bloqueando el lote ante cualquier conflicto. No cubre sustitución, actualización ni efectos que requieran respaldo completo (spec 010).
- Evidencia Rust 2026-09-30: `cargo check --manifest-path src-tauri/Cargo.toml --locked` y prueba unitaria del modelo del plan CSV pasaron. `mysql_csv_batch_applies_atomically_and_compensation_blocks_external_changes` compila como integración ignorada y prueba aplicación, colisión entre preparación/aplicación sin inserción parcial, bloqueo ante edición externa y compensación completa cuando se restablecen los valores. No se ejecutó por falta de servidor/credencial de prueba declarados en el entorno.
- **IPC Tauri en Windows (2026-09-30):** tests de integración ejecutan comandos de producción por `MockRuntime`; confirmación con ID inválido devuelve `INVALID_HISTORY_REVISION`. El round-trip de edición y reversión pasó contra MySQL Community 8.4.11 desechable: invocó preparación, confirmación, aplicación y compensación usando artefactos cifrados e historial local aislado, y comprobó que solo aplicar cambia la fila. También pasó importación y compensación CSV por el mismo harness. Esto acredita el IPC del core con servidor real, pero falta la ventana Tauri nativa y la matriz completa de escrituras/permisos/versiones.

## Unidad de trabajo

Un proyecto versionado identifica una conexión y una base de datos de destino. DBSUAL crea el historial local al preparar el primer cambio para ese destino; crear una base nueva también inicia su historial antes de ejecutarse. Un conjunto de cambios pertenece a una sola base de datos y puede contener varias operaciones ordenadas. El historial permanece disponible aunque la base se elimine. Conserva, como mínimo:

- Identificador, fecha, autor y mensaje de la revisión.
- Motor y referencia no secreta del destino.
- Operaciones preparadas y orden de aplicación.
- Resumen de diferencias de estructura y datos que se pudo comprobar.
- Estado: **Borrador**, **Confirmado sin aplicar**, **Aplicando**, **Aplicado**, **Fallido**, **Fallido parcialmente** o **Estado incierto**. El detalle de cada paso y los conflictos se rigen por el [spec 015](015-conflictos-y-aplicacion.md).
- Identificador y estado del punto de recuperación anterior a la aplicación.
- Resultado de cada operación y verificaciones posteriores.

Las credenciales no forman parte del historial. Los datos sensibles contenidos en artefactos y respaldos requieren almacenamiento protegido.

## Flujo principal

1. Al preparar el primer cambio, DBSUAL crea o abre el historial local del destino. Las consultas de lectura y las exportaciones no necesitan preparar una revisión. Crear una base vacía usa un proyecto ligado al nombre futuro y no requiere respaldo previo.
2. Acciones de DBSUAL como crear, importar o eliminar se agregan a **Cambios preparados** en lugar de ejecutarse inmediatamente. En el caso de SQL escrito por la persona, la aplicación distingue lo que puede analizar de lo que no puede previsualizar con certeza.
3. La vista previa muestra destino, operaciones, objetos afectados, posibles pérdidas de datos y diferencias conocidas. Si el alcance no se puede determinar, se indica explícitamente.
4. La persona escribe un mensaje y confirma una revisión local con el plan. Puede seguir revisándola antes de aplicarla.
5. Al elegir **Aplicar a la base**, DBSUAL comprueba que el destino sigue siendo el esperado y que no hubo cambios externos incompatibles desde la vista previa. Si los detecta, detiene la aplicación y pide volver a revisar.
6. Antes de aplicar, crea y verifica el punto de recuperación requerido por [010 — Respaldos y recuperación](010-respaldos-y-recuperacion.md): compensación por cambio localizado o copia completa para operaciones destructivas o de efecto incierto. Si no puede hacerlo, no aplica el cambio.
7. Aplica las operaciones, registra los resultados reales y vuelve a inspeccionar el destino. Solo marca **Aplicado** cuando pudo comprobar el resultado.
8. El resultado de aplicación se registra como un evento interno vinculado a la revisión. La aplicación no cambia ni reescribe el plan original.

## Integración Git opcional

- DBSUAL puede exportar cambios de estructura y scripts revisables a un repositorio Git elegido por la persona.
- El repositorio Git no recibe credenciales, filas de datos ni respaldos completos en texto claro por defecto.
- **Subir a Git** comparte los archivos exportados con un remoto configurado; no aplica cambios a ninguna base de datos.
- Un clon del repositorio Git no equivale a clonar el historial completo de DBSUAL ni garantiza restaurar una base. La sincronización del historial y de los respaldos requiere un diseño separado.
- La falta de Git o de un remoto configurado no impide preparar, aplicar o restaurar cambios mediante el historial propio de DBSUAL.

## Recuperación

- **Descartar:** retira cambios preparados que todavía no se han aplicado; no toca la base de datos.
- **Cancelar aplicación en curso:** se intenta detener la ejecución. La interfaz explica qué operaciones se completaron y cuáles quedaron inciertas; cancelar no implica reversión automática.
- **Revertir un cambio aplicado:** crea una nueva revisión compensatoria cuando sea posible y la somete al mismo proceso de revisión. No reescribe el historial.
- **Restaurar punto anterior:** recupera una copia verificada de la base completa. Antes de hacerlo, muestra qué datos actuales se reemplazarán, comprueba cambios externos y crea un punto de recuperación del estado actual cuando sea posible. Restaurar en una base nueva también pasa por una revisión cifrada, confirmación explícita y evento de aplicación en el historial.
- La disponibilidad de cada opción se muestra por cambio. La palabra **Rollback** solo se usa para una transacción activa que el motor pueda revertir; no se ofrece como promesa genérica después de aplicar.

## Diferencias entre motores

- MySQL puede confirmar implícitamente operaciones de definición de datos como `CREATE DATABASE` y `DROP DATABASE`; por eso una aplicación de varios pasos puede quedar parcialmente ejecutada y su recuperación depende de un respaldo o de un cambio compensatorio.
- PostgreSQL tampoco permite ejecutar `DROP DATABASE` dentro de un bloque de transacción. Una eliminación ya confirmada requiere restauración desde respaldo.
- SQLite admite operaciones transaccionales dentro del archivo, pero borrar el archivo de la base de datos requiere un punto de recuperación del archivo antes de eliminarlo.
- El proyecto no promete una transacción única entre varios motores, varias bases de datos o el sistema de archivos y un servidor remoto.

## Respaldo de bases completas

- Para permitir recuperar eliminaciones o importaciones destructivas, el punto de recuperación debe abarcar la estructura y los datos completos de la base afectada. No basta con guardar únicamente el SQL de la operación. Las actualizaciones localizadas pueden usar imágenes anteriores de las filas bajo las reglas del spec 010.
- El respaldo debe tener una comprobación de integridad y metadatos del motor y versión. Antes de depender de un formato de respaldo para un motor, se prueba una restauración representativa.
- Se muestra una estimación de tamaño y tiempo cuando sea posible. Si no hay espacio, permisos o capacidad para generar un respaldo íntegro, la aplicación detiene la aplicación protegida y explica el motivo.
- Los respaldos se cifran en reposo y su retención se configura explícitamente. El historial no elimina automáticamente el último punto necesario para recuperar un cambio aplicado.
- Una revisión debe seguir siendo legible aunque falte un respaldo, pero la interfaz marca **Recuperación no disponible** para ese cambio hasta que se encuentre y verifique el artefacto correspondiente.

## Criterios de aceptación iniciales

1. Dada una eliminación MySQL preparada, la base sigue intacta después de preparar y confirmar el cambio; solo se elimina al elegir **Aplicar a la base**.
2. Antes de aplicar esa eliminación, se muestra el destino exacto, la pérdida prevista y el estado del punto de recuperación. Si el respaldo íntegro falla, no se envía `DROP DATABASE`.
3. Dado un cambio externo incompatible entre vista previa y aplicación, DBSUAL bloquea la aplicación hasta volver a inspeccionar el destino.
4. Dado un cambio aplicado correctamente, el historial registra el resultado y permite identificar el respaldo anterior.
5. Dado un fallo a mitad de una aplicación no transaccional, el historial registra operaciones completadas, fallidas e inciertas sin declarar una reversión que no ocurrió.
6. Dado un punto de recuperación válido, restaurar exige revisar el destino y crea un nuevo evento de historial; no borra la revisión original.
7. Quitar una conexión guardada no borra el historial ni los respaldos de un proyecto versionado sin una acción separada y explícita.
8. Dado un proyecto sin Git configurado, preparar, confirmar, aplicar y restaurar cambios sigue funcionando.
9. Dado un punto de recuperación cuyo respaldo cifrado falta o falla la verificación, el historial se puede consultar pero la opción de restaurar ese punto permanece deshabilitada y explica qué artefacto falta.
10. Dado un repositorio Git opcional, **Subir a Git** publica solo los archivos exportados pendientes y no ejecuta operaciones en ninguna base de datos.
11. Si no se puede crear o escribir el historial local de un destino, cualquier acción que lo modifique se bloquea antes de enviar instrucciones al motor.
12. Preparar y confirmar una restauración en base nueva no crea bases; solo una revisión confirmada puede iniciarla. El historial conserva el resultado aplicado, fallido o incierto, y una repetición sobre una revisión ya iniciada se rechaza.
13. Si la aplicación termina con una revisión en `applying`, el siguiente arranque la marca `uncertain` y agrega un evento una sola vez; no repite automáticamente la operación remota.
14. Una revisión `uncertain` se identifica en español y advierte que hay que comprobar el destino; abrirla no ejecuta aplicación ni ofrece repetirla automáticamente. Los planes de actualizar/revertir fila presentan una diferencia legible sin revelar el JSON interno.

## Decisiones pendientes

1. Margen de espacio libre para respaldos locales. En el primer MVP la retención es manual y la sincronización entre equipos queda fuera.
2. Nivel de detalle de diferencias de datos antes de aplicar cambios grandes.
3. Matriz exacta de SQL arbitrario admitido por motor; el criterio general de clasificación y conflictos se define en el spec 015.
4. Formato de exportación de cambios de estructura y scripts a Git y relación con las revisiones internas.

## Fuentes técnicas que condicionan la recuperación

- [MySQL: instrucciones que causan commit implícito](https://dev.mysql.com/doc/refman/8.0/en/implicit-commit.html).
- [PostgreSQL: `DROP DATABASE` no puede ejecutarse dentro de una transacción](https://www.postgresql.org/docs/15/sql-dropdatabase.html).
- [SQLite: API de respaldo de una base activa](https://sqlite.org/backup.html).

**Reversión de edición desde Historial (2026-09-30):** Historial ahora ofrece «Preparar reversión» únicamente para revisiones MySQL de edición/reversión de edición con estado `applied` y recuperación `verified`; excluye inserciones, borradores, resultados inciertos y otros motores. Requiere que la conexión ya esté abierta manualmente. La preparación llama `prepare_mysql_row_revert` y abre la diferencia de una revisión nueva; confirmar conserva el diálogo y solo actualiza el estado local visible. La escritura ocurre únicamente al pulsar «Aplicar reversión», que invoca `apply_mysql_row_update`. Tras el éxito, App solicita recarga de las pestañas cuya conexión, base y tabla coinciden con el cambio. `tests/e2e/history-row-revert.spec.ts` verifica con IPC simulado que preparación y confirmación no mutan, la aplicación requiere esa acción explícita y la cuadrícula vuelve a leer el valor restaurado. Esto valida la interfaz; el comando Tauri y el servidor real siguen pendientes.

**Importación CSV validada por comandos Tauri (2026-09-30):** la preparación, confirmación, aplicación y compensación se invocaron mediante Tauri MockRuntime contra MySQL Community 8.4.11 desechable; preparación/confirmación no escriben y la reversión exige su propia aplicación confirmada. Consulta la evidencia y sus límites en el spec 004. Aún falta validar el mismo flujo desde la ventana Tauri nativa.
