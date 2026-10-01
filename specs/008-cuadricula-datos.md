# Spec 008 — Vista y edición de filas

Estado: cuadrículas de solo lectura para PostgreSQL y SQLite; edición e inserción protegidas MySQL limitadas con integración de core real
Versión: 1.2
Última actualización: 2026-09-30
Depende de: [002 — Explorador](002-explorador.md), [005 — Interfaz](005-interfaz-de-trabajo.md), [006 — Control de cambios](006-control-de-cambios.md)

## Objetivo

Permitir inspeccionar, añadir, editar y eliminar filas de una tabla desde una cuadrícula, sin escribir SQL manualmente. Toda modificación sigue el flujo obligatorio de preparar, revisar, confirmar una revisión y aplicar. MySQL se implementará primero, seguido de MariaDB, PostgreSQL y SQLite.

## Abrir y recorrer datos

1. Al abrir una tabla desde el explorador, la pestaña muestra su conexión, base de datos, esquema cuando corresponda y nombre de tabla.
2. El nodo de una tabla ofrece **Ver datos**, que abre una cuadrícula de esa tabla sin requerir que la persona escriba SQL. La cuadrícula es un explorador de datos separado de la ficha de estructura del spec 002 y del editor SQL del spec 007.
3. Cada tabla se abre en su propia pestaña de datos. Pueden permanecer abiertas varias cuadrículas de tablas distintas al mismo tiempo, incluso de bases de datos o conexiones distintas.
4. Abrir datos de otra tabla no reemplaza ni cambia el destino de las cuadrículas ya abiertas. Cada pestaña mantiene su conexión, base de datos, esquema y tabla; sus filas, errores y estado de carga son independientes.
5. Si ya existe una pestaña para la misma tabla y contexto, **Ver datos** la activa en vez de abrir un duplicado. La identidad considera motor, conexión, base de datos, esquema y tabla.
6. La persona puede cerrar una pestaña de datos sin cerrar las demás. Si más adelante esa pestaña contiene cambios sin aplicar, se aplican las reglas de confirmación de cierre de los specs 005 y 006.
7. La cuadrícula carga filas por páginas o lotes mediante desplazamiento infinito; al acercarse al final del tramo visible solicita automáticamente el siguiente, sin botón «Cargar más».
7a. El presupuesto nominal de 1 MB se evalúa en fronteras de fila para reducir cuántas filas se envían por lote; nunca rechaza la tabla por excederlo. Una fila se entrega completa incluso si por sí sola excede el presupuesto, y la paginación continúa desde la siguiente fila sin truncar celdas.
7b. El pie muestra el total real y localizado con el formato «400 de 11.938 registros», respetando filtros activos. Mientras carga otra página, conserva el conteo. Al llegar al total muestra «No hay más registros».
8. La persona puede ordenar y filtrar por columnas. Las operaciones se aplican en el servidor o en el archivo SQLite, no solo al subconjunto visible, y la interfaz indica filtros y orden activos.
9. Los valores `NULL`, cadena vacía y valor predeterminado se representan de forma distinta.
10. PostgreSQL y SQLite son explícitamente de **solo lectura**. MySQL habilita solo edición e inserción en las tablas elegibles descritas abajo, siempre por el historial; la eliminación ordinaria permanece deshabilitada. Volver a consultar o actualizar la lectura es una acción de refresco, no escritura.
11. Los tipos que no tengan una representación de lectura segura se muestran como no disponibles o de forma segura, sin conversiones silenciosas.
12. Una carga tardía o fallida no reemplaza datos de otra tabla o conexión abiertos en una pestaña diferente.

La cuadrícula ofrece lectura en todos los motores implementados. Su barra de navegación debe dejar claro el objeto y permitir recorrer los datos sin pasar por la ficha de estructura. Poder abrir una tabla no basta para habilitar escritura: cada operación tiene que cumplir su flujo protegido y sus criterios específicos.

## Tamaño de columnas y lectura de valores largos

- Cada columna de la cuadrícula se puede redimensionar arrastrando el separador de su encabezado, de forma similar a cambiar el ancho de los paneles de DBSUAL. El área de arrastre tiene un cursor de cambio de tamaño y una indicación visual clara al enfocarse o arrastrarse; no activa la ordenación del encabezado.
- Las columnas tienen un ancho mínimo que mantiene legibles sus datos y un máximo que evita desplazar fuera de vista toda la tabla. Se permite desplazamiento horizontal cuando el conjunto de anchos supera el área disponible; el redimensionado no reduce el tamaño de fuente ni oculta columnas silenciosamente.
- Los anchos se conservan en la pestaña de esa tabla al paginar, ordenar, filtrar o refrescar. Cada pestaña mantiene su propio ajuste, incluso si otra tabla tiene columnas con los mismos nombres. El ajuste inicial se obtiene de un ancho razonable según el encabezado, sin esperar a leer toda la tabla.
- El alto de una fila no crece sin límite por el contenido de una celda. De forma predeterminada, el texto multilínea se limita visualmente a unas pocas líneas y señala que hay contenido adicional, sin alterar el valor original.
- Al activar una celda truncada con clic o teclado, DBSUAL revela su contenido completo en una expansión acotada y desplazable, asociada a esa celda. La expansión no cambia otras filas ni modifica o descarta el valor original; se puede cerrar o contraer de forma obvia. Los valores normales y `NULL` mantienen sus representaciones existentes.
- La expansión permite seleccionar y copiar el valor completo. Se puede cerrar con Escape, al activar otra celda expandible o al cambiar de pestaña; no intercepta el control de edición de una celda ni activa ordenamiento.
- Los tiradores y la expansión funcionan con teclado y exponen nombre de columna, ancho y estado accesibles. En pantallas estrechas se conservan los anchos mínimos y el scroll horizontal, en lugar de comprimir todas las columnas hasta hacerlas ilegibles.

## Menú contextual de celdas y portapapeles

- Al hacer clic derecho sobre el valor de una celda, DBSUAL abre su menú contextual propio y ofrece **Copiar** el valor completo de esa celda. Esto también funciona cuando el valor está truncado visualmente o expandido; se copia el contenido original, no la representación recortada.
- Si el clic derecho ocurre sobre el editor inline activo de una celda, el menú ofrece las acciones de texto aplicables (**Copiar**, **Cortar**, **Pegar** y **Seleccionar todo**) según selección, contenido del portapapeles y permisos del editor. El foco y la selección de texto se conservan al abrir el menú.
- **Pegar** solo está disponible cuando hay un editor inline activo y habilitado. Pega el texto como propuesta en el editor de la celda; nunca modifica directamente la base ni omite preparar, revisar, confirmar y aplicar. La acción queda sujeta a las mismas validaciones de tipo y límites que escribir en el editor.
- Fuera de un editor activo, la cuadrícula es de lectura respecto al portapapeles: ofrece copiar y no ofrece pegar. En motores o tablas de solo lectura, pegar no aparece como acción ejecutable.
- Para `NULL`, copiar copia una cadena vacía y no el texto literal `NULL`; el estado nulo continúa distinguiéndose visualmente y solo cambia mediante el control explícito de NULL del editor.
- Si el portapapeles del sistema no está disponible o el permiso de lectura falla, DBSUAL muestra un error breve y no borra ni reemplaza la propuesta actual. No guarda valores copiados en logs, historial ni almacenamiento persistente.
- Las acciones se habilitan/deshabilitan según el estado real del editor y del portapapeles, son accesibles por teclado y mantienen suprimir el menú nativo del WebView conforme al spec 005.

## Editar filas existentes

- La cuadrícula permite editar una celda o varias celdas de una fila identificable.
- Para habilitar edición, la tabla debe tener una clave primaria o una clave única estable que identifique exactamente una fila. Se admiten claves compuestas. Si no existe una clave adecuada, la cuadrícula es de solo lectura.
- Las vistas son de solo lectura en el MVP, aunque el motor admita vistas actualizables.
- Las ediciones quedan como borrador local y se distinguen visualmente del valor vigente en la base.
- Antes de confirmar una revisión se muestra tabla, clave de fila, valores anteriores y nuevos, restricciones conocidas y número de filas previstas.
- Confirmar la revisión no escribe en la base. **Aplicar a la base** vuelve a verificar que la fila y sus valores relevantes no cambiaron externamente. Si hay conflicto, no sobrescribe la fila y pide revisar.
- El core usa parámetros para valores y construye identificadores de tabla y columna según las reglas del motor; el texto de una celda nunca se ejecuta como SQL.
- Tras aplicar, la cuadrícula vuelve a consultar la fila. El éxito se muestra solo cuando el motor confirma la operación y el resultado puede verificarse.

## Añadir filas

- La persona puede crear una fila nueva en la cuadrícula. Los campos obligatorios se distinguen de los campos que el motor puede completar por defecto.
- La fila nueva permanece como borrador hasta confirmar una revisión. La vista previa muestra valores proporcionados y valores que se dejarán al motor.
- **Aplicar a la base** inserta la fila y vuelve a consultarla, incluidos identificadores y valores generados por el motor. El historial registra el resultado real.
- Si la inserción falla por tipo, restricción o permiso, el borrador permanece disponible para corregirlo y no se presenta como fila guardada.

## Eliminar filas

- La persona puede marcar una o varias filas identificables para eliminación. La cuadrícula las diferencia de filas ya eliminadas en la base.
- La vista previa indica tabla, claves de las filas, cantidad, efectos conocidos y si puede haber relaciones, disparadores o cascadas. La ausencia de información sobre efectos secundarios se declara como incertidumbre.
- Antes de aplicar se crea y verifica el punto de recuperación exigido por `006-control-de-cambios.md`. Si falla, no se envía la eliminación.
- **Aplicar a la base** vuelve a comprobar identidad y estado de las filas. Si alguna cambió externamente, bloquea esas eliminaciones y pide revisión en lugar de borrar por posición visual.
- Después de aplicar, la cuadrícula actualiza las filas y el historial registra el resultado. Restaurar filas eliminadas se trata como un nuevo cambio; si hubo efectos secundarios, se requiere el respaldo completo para recuperar la base con fidelidad.

## Recuperación de cambios en filas

- El historial conserva los valores anteriores necesarios para preparar una revisión compensatoria, cifrados cuando contienen datos sensibles.
- **Revertir** una edición aplicada prepara un nuevo cambio que vuelve a colocar los valores anteriores, sujeto a la misma comprobación de conflictos. No borra la revisión original.
- **Revertir** una inserción aplicada prepara la eliminación de la fila creada, tras comprobar que sigue siendo la misma y que no hay efectos secundarios desconocidos. Si no puede comprobarlo, la restauración desde respaldo es la vía segura.
- Si disparadores, cascadas u otros efectos pueden modificar datos fuera de las filas identificadas, la interfaz no promete que una reversión de filas restaure toda la base. Se necesita un respaldo completo verificable para ofrecer restauración completa.
- Las operaciones cuya recuperación no pueda garantizarse se señalan antes de aplicar, de acuerdo con el spec de control de cambios.

## Estados y errores

- Los errores de validación se muestran junto a la celda afectada cuando sea posible.
- Un error de permisos, restricción o conflicto identifica las filas afectadas sin ocultar los demás borradores.
- Si se pierde la conexión durante la aplicación, la revisión queda en estado **Estado incierto** hasta volver a consultar las filas afectadas.
- Cambiar de pestaña o cerrar la aplicación no descarta silenciosamente ediciones preparadas o sin confirmar.

## Criterios de aceptación iniciales: MySQL

1. Abrir una tabla con más filas que el primer lote muestra solo una parte, indica que hay más y permite continuar cargando sin congelar la interfaz.
2. Ordenar o filtrar vuelve a consultar el origen y no da a entender que solo ordenó o filtró las filas ya visibles.
3. Editar una celda de una tabla con clave primaria crea un borrador; preparar y confirmar la revisión no cambia el valor almacenado hasta **Aplicar a la base**.
4. Si otra sesión cambia la fila antes de aplicar, la edición se bloquea con un conflicto y no sobrescribe silenciosamente el cambio externo.
5. Una tabla sin clave única adecuada y una vista se muestran en modo de solo lectura.
6. Después de aplicar una edición, el historial muestra valores anteriores y nuevos de manera protegida y permite preparar una revisión compensatoria cuando es viable.
7. Los valores `NULL` y cadena vacía siguen siendo distintos al editar, previsualizar y volver a consultar.
8. Añadir una fila con valores válidos crea un borrador; la fila solo aparece como persistida tras aplicar y verificar la inserción.
9. Marcar una fila para eliminar no la borra; si el punto de recuperación falla, **Aplicar a la base** no envía la eliminación.
10. Si otra sesión modifica una fila marcada para eliminar, la aplicación informa el conflicto y no elimina una fila diferente ni usa la posición visual como identificador.
11. Abrir **Ver datos** en dos tablas distintas deja dos pestañas de cuadrícula disponibles; cambiar entre ellas conserva cada destino y no mezcla filas ni estados de carga.
12. Volver a abrir desde el árbol una tabla cuya pestaña ya está abierta activa esa pestaña según la identidad de conexión, base, esquema y tabla, sin crear un duplicado.
13. Para motores sin escritura protegida disponible y filas no identificables, no existe acción habilitada para escribir desde la cuadrícula. MySQL presenta edición limitada e inserción protegida únicamente en tablas elegibles InnoDB con clave primaria; PostgreSQL y SQLite siguen en solo lectura y la eliminación ordinaria permanece bloqueada.
14. El ancho de una columna se puede cambiar arrastrando su límite en el encabezado, sin activar su ordenamiento; el ancho mínimo, máximo y scroll mantienen utilizable la cuadrícula.
15. El ajuste de ancho permanece al filtrar, ordenar, paginar o refrescar esa tabla y es independiente en cada pestaña de datos.
16. El texto largo de una celda no aumenta ilimitadamente la altura de fila: aparece truncado con una señal visible y puede expandirse de manera acotada para consultar el valor íntegro.
17. La expansión muestra y permite seleccionar/copiar el valor completo, se puede contraer con mouse o teclado y no altera ni el contenido del servidor ni el flujo de edición protegida.
18. Con teclado se puede enfocar el control para redimensionar una columna y ajustar su ancho; el ancho actual y la acción se anuncian de forma accesible.
19. Clic derecho en una celda ofrece copiar su valor completo, incluso si se muestra truncado; el texto literal `NULL` no se confunde con el valor nulo.
20. Clic derecho en el editor inline permite pegar el texto disponible en el portapapeles como propuesta. No hay acción Pegar habilitada sin un editor activo, y pegar nunca aplica cambios a la base.
21. Copiar, cortar y seleccionar todo respetan la selección y el foco del editor; fallos de acceso al portapapeles se informan sin descartar el valor que se estaba editando.
22. Las opciones contextuales de celda/editor son propias de DBSUAL, accesibles por teclado y no muestran el menú nativo del WebView.

## Extensión PostgreSQL de solo lectura

- La tabla se identifica por conexión, base, esquema y nombre. Dos esquemas pueden contener tablas homónimas sin mezclar sus filas ni pestañas.
- La lectura usa una conexión a la base seleccionada, valida el objeto y las columnas visibles, y solo admite tablas o vistas del catálogo PostgreSQL.
- Orden y filtro se validan contra las columnas del objeto. Los valores del filtro se enlazan como parámetros; los nombres de esquema, tabla y columna se escapan como identificadores PostgreSQL.
- Cada lote devuelve hasta 200 filas, conserva `NULL`, impone timeout de 15 segundos y ejecuta dentro de una transacción de solo lectura. El desplazamiento máximo es 1 000 000.
- Los errores de conexión, permisos, tipo de objeto, columna inválida y timeout se muestran como error; no se convierten en una tabla vacía.
- Verificada con PostgreSQL 16 desechable usando dos esquemas con tablas homónimas, filtro con caracteres SQL/metacaracteres, valores `NULL` y recorrido desde la UI con IPC simulado. La ventana Tauri nativa, TLS y matriz de versiones/permisos siguen pendientes.

### Puerta de salida para habilitar edición PostgreSQL

La edición PostgreSQL permanece deshabilitada. Que la tabla tenga clave primaria no basta para habilitarla: primero debe existir un proveedor de respaldo PostgreSQL probado que genere un punto completo, cifrado, verificado y ligado a la revisión, y una aplicación que conserve conflictos y estados inciertos. El spec 012 aún declara que no existe ese proveedor; por tanto, preparar, confirmar o aplicar una edición desde la cuadrícula no está disponible.

La primera porción futura debe limitarse a tablas ordinarias con PK, columnas escalares admitidas, sin triggers ni políticas RLS/efectos que el adaptador no pueda acotar; excluir vistas, particiones inicialmente y columnas generadas. Debe comparar una huella de fila obtenida bajo bloqueo transaccional al aplicar, ejecutar actualización condicionada por PK y valores originales, comprobar que afecta exactamente una fila y volver a leer el resultado antes de marcar `applied`. Los valores van enlazados como parámetros y los identificadores/type casts se obtienen del catálogo y se escapan como identificadores PostgreSQL. Si no se puede verificar recuperación completa, conflicto o resultado, la escritura sigue bloqueada.

Antes de habilitar la operación deben pasar pruebas con PostgreSQL desechable que demuestren que preparar y confirmar no escriben, la aplicación confirmada cambia una sola fila, un cambio concurrente causa conflicto sin sobrescribir, el resultado incierto no se reintenta automáticamente y la compensación se prepara como una revisión nueva. También se requiere recorrido del comando Tauri con el artefacto cifrado real.

## Decisiones pendientes

1. Tipos de datos con edición especializada en la primera entrega, como JSON, fecha/hora y binarios.
2. Límite inicial de filas por lote y opciones de filtrado avanzado.
3. Definir y validar el acceso al portapapeles del sistema en cada WebView nativo soportado, manteniendo el error explícito si el permiso no está disponible.

## Estado actual de implementación

- El editor SQL permite ejecutar lecturas `SELECT` acotadas y consultar resultados por lotes; esto permite inspección mediante consultas escritas.
- Las cuadrículas de solo lectura se abren desde **Ver datos** y conservan una vista independiente por identidad de conexión, base y tabla. Abrir otra tabla mantiene ambas; repetir la misma identidad activa la existente; cerrar la activa selecciona otra abierta o Inicio. Cada vista conserva su paginación, carga, error y destino mientras permanece montada.
- La prueba Playwright comprueba dos pestañas, alternancia, cierre y reapertura sin duplicado mediante eventos de apertura de tabla en preview. Esta prueba no ejecuta el IPC de una app Tauri ni compara filas de un servidor real.
- El IPC `read_table_page` vuelve a consultar el origen para ordenar y aplicar un filtro de igualdad o contiene. Rust valida las columnas contra `information_schema`, enlaza el valor del filtro como parámetro y añade la clave primaria como desempate para recorrer páginas de forma determinista. La página queda limitada a 200 filas y conserva el límite de offset, timeout y cancelación del lector existente.
- El límite nominal de 1 MB ahora ajusta el tamaño de cada lote en fronteras de fila; no rechaza una tabla ni trunca sus celdas. Una fila individual de mayor tamaño se devuelve completa y puede hacer que ese lote supere 1 MB. Compilado en Windows; falta comprobarlo con datos grandes en un servidor real.
- MySQL Community 8.4.11 y MariaDB 10.6.28, 10.11.19 y 11.4.13 comprobaron orden con nulos y empate por clave primaria a través del límite de página, identificadores con acento grave, filtro cuyo texto parece SQL y rechazo de una columna no existente. Playwright verifica los controles, paginación y limpieza del filtro; también confirma que el valor se envía como dato IPC, no como SQL. Falta confirmar el flujo desde una app Tauri nativa y la paginación real bajo escrituras concurrentes; las vistas sin clave única tampoco garantizan orden estable.
- La edición inline protegida se añadió solo para MySQL: permite proponer una celda no clave cuando la fila tiene PK y el core confirma tabla InnoDB, tipos escalares admitidos y ausencia de triggers. El modal separa preparación, confirmación y aplicación; prepara compensación como una nueva revisión tras una aplicación. La UI evita habilitar otros motores y muestra que preview web no tiene core. E2E verifica IPC simulado, que preparar/confirmar no aplican, que una revisión cerrada puede reabrirse y que la compensación también necesita confirmar y aplicar. Una integración del core pasó contra MySQL Community 8.4.11 y verificó edición aplicada y rechazo de conflicto externo. Aún falta el recorrido del comando Tauri con artefacto cifrado y una reversión completa desde el comando nativo.
- PostgreSQL continúa solo lectura. No se añadió ni verificó una ruta de escritura: su proveedor de respaldo está pendiente en el spec 012, así que no se puede cumplir la recuperación requerida antes de aplicar. Una utilidad compartida de escape de identificadores PostgreSQL quedó ubicada en el adaptador PostgreSQL y mantiene la prueba unitaria de comillas; esto no habilita escritura.
- Implementación inicial del redimensionamiento de columnas y lectura expandible de valores largos en la cuadrícula React: divisores arrastrables y accesibles por teclado, ancho independiente por columna dentro de cada pestaña, truncamiento visual a tres líneas y contenido expandido con scroll interno hasta 240 px o 40 vh. Cambiar de vista o recargar cierra la expansión, sin tocar el valor; anchos y valores sobreviven mientras la pestaña siga abierta. `npm run build` pasó; falta verificación visual y de interacción en Windows/Playwright, incluidos datos muy largos y columnas con nombres duplicados.
- Implementación inicial del menú contextual de celda/editor en `src/TableDataView.tsx`: clic derecho en una celda copia el valor completo (para `NULL`, una cadena vacía); en el editor inline permite copiar/cortar la selección, pegar desde el portapapeles del sistema y seleccionar todo. Pegar modifica solo el borrador local y conserva la selección/caret; no escribe en la base. Los errores de permisos se muestran sin reemplazar la propuesta. Falta verificar el acceso real al portapapeles y el foco en las ventanas Tauri nativas de Windows, macOS y Linux.

## Total de registros y desplazamiento infinito — 2026-09-30

La cuadrícula solicita el total de registros para MySQL, PostgreSQL y SQLite, aplicando el mismo filtro enlazado que la lectura paginada. El pie presenta los conteos con formato local (`400 de 11.938 registros`) y anuncia cuando se llegó al final. El siguiente tramo se carga automáticamente al aproximarse al final del contenido, con margen anticipado de 5.000 px; el total se conserva al anexar páginas. Se verificó la compilación TypeScript/Vite y `cargo check --locked`; no se recorrió la interfaz con servidores reales. El total puede variar si hay escrituras concurrentes en el origen.

## Avance de cuadrícula SQLite — 2026-09-30


SQLite ya permite consultar tablas en modo de solo lectura con orden, filtros enlazados, `NULL` diferenciado, blobs en hexadecimal y lotes limitados por filas y tamaño nominal. La cuadrícula no habilita editar, insertar ni eliminar registros: esos cambios esperan el flujo del historial y la recuperación SQLite validada por el spec 013. Falta la verificación de la ventana Tauri nativa.

## Avance de inserción protegida MySQL — 2026-09-30

La cuadrícula permite proponer valores para **todas** las columnas de una tabla MySQL elegible, incluyendo la PK, y distingue NULL. Preparar y confirmar guardan el plan cifrado y la compensación local sin mutar la base; aplicar vuelve a comprobar servidor, esquema y ausencia de la clave bajo transacción, inserta con parámetros y verifica los valores mediante una lectura textual explícita. Una aplicación exitosa se puede revertir preparando una revisión nueva de eliminación: compara todas las columnas capturadas y su huella bajo bloqueo antes de eliminar. La inserción y su compensación se limitan a tablas base InnoDB con PK, tipos escalares soportados, sin columnas generadas ni `ON UPDATE`, triggers ni FKs entrantes. Se rechazan valores incompletos, claves ya existentes, conflictos y cambios externos; la UI de preview web no habilita la acción.

Verificación: E2E Playwright con IPC simulado recorrió preparar, revisar, confirmar, aplicar inserción y compensación. La integración del core contra MySQL Community 8.4.11 desechable recorrió inserción y reversión con artefactos cifrados autenticados mediante el lector del historial. Con fixtures reales, un trigger `BEFORE INSERT` bloqueó inserción y reversión; una FK entrante `ON DELETE CASCADE` también bloqueó ambas y dejó la fila hija; una edición externa de la fila insertada hizo fallar la preparación compensatoria con `ROW_REVERT_CONFLICT` y preservó el valor externo. El mismo recorrido completo pasó contra MySQL Community 8.0.46 desechable. Las integraciones usan historial aislado e invocan el core directamente: no validan el comando Tauri ni el IPC nativo. No se habilita inserción en MariaDB, PostgreSQL o SQLite. La eliminación ordinaria de filas sigue sin estar disponible.

**Acceso a compensación desde Historial (2026-09-30):** además del control temporal de reversión junto a la cuadrícula, Historial expone preparar una nueva revisión compensatoria para una edición MySQL aplicada y recuperable, lo que permite revisarla fuera de la pestaña de datos. Se requiere abrir la conexión explícitamente; la preparación y confirmación no escriben. Tras aplicar, App recarga únicamente las cuadrículas abiertas para la misma conexión, base y tabla. La E2E `history-row-revert.spec.ts` cubre estados y acciones con IPC simulado y comprueba que la cuadrícula vuelva a consultar el valor revertido. No habilita reversión de inserciones desde Historial, edición/eliminación ordinaria, otros motores ni demuestra aplicación desde la ventana Tauri nativa.
