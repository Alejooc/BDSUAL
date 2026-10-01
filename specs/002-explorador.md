# Spec 002 — Explorador de bases de datos, tablas y vistas

Estado: explorador MySQL/MariaDB parcial; catálogo y columnas PostgreSQL implementados; exploración SQLite de solo lectura
Versión: 0.9  
Última actualización: 2026-09-29  
Depende de: [001 — Conexiones](001-conexiones.md)

## Objetivo

Mostrar, dentro de cada conexión abierta, las bases de datos accesibles, sus tablas y vistas, y las columnas de cada una para que la persona pueda encontrar los objetos con los que va a trabajar. MySQL se implementará primero; MariaDB, PostgreSQL y SQLite seguirán el orden acordado en el spec de visión.

## Alcance

- Mostrar las conexiones guardadas y su estado.
- Permitir mantener varias conexiones guardadas, cada una con sus propias acciones y estado independientes; **Conectar**/**Desconectar** es la acción primaria y **Editar**/**Quitar** se agrupan en **Más opciones**.
- Expandir una conexión abierta para listar las bases de datos a las que el usuario tiene acceso.
- Mostrar bajo cada conexión las secciones de servidor **Procesos**, **Usuarios** y **Variables** como nodos separados de las bases de datos, cuando el motor las soporte.
- Expandir una base de datos para listar sus tablas y vistas en grupos distintos.
- Expandir una tabla o vista para listar sus columnas con nombre, tipo de dato, nulabilidad, pertenencia a clave primaria y valor predeterminado cuando esos metadatos estén disponibles.
- Abrir una tabla en una vista de datos tabular y abrir su estructura detallada desde el explorador. Son acciones distintas de expandir el nodo para ver sus columnas.
- El clic derecho sobre conexiones, bases de datos, esquemas, tablas, vistas y columnas abre el menú contextual de DBSUAL para ese tipo de objeto, según el spec 005. No muestra el menú del navegador.
- La estructura detallada de una tabla incluye, según los metadatos disponibles del motor, columnas y sus atributos, claves primarias y únicas, índices, restricciones, claves foráneas y relaciones. La interfaz distingue metadatos no disponibles de una lista vacía.
- Cada tabla ofrece **Propiedades**, con secciones para General, Información, Llaves, Campos, Llaves foráneas, Triggers, Particiones, Dependencias, Extras y Código fuente, según lo que admita el motor y el objeto.
- Actualizar manualmente la lista de bases de datos, tablas y vistas, o columnas de un objeto.
- Indicar carga, lista vacía y error sin perder de vista el resto del árbol.

## Interacción

1. Una conexión guardada aparece en el panel explorador incluso cuando está desconectada.
2. Cada conexión aparece en una fila identificable con estado y acción primaria **Conectar**/**Desconectar**. Su estado es independiente de las demás conexiones guardadas.
3. El botón **Más opciones** (tres puntos) de una conexión presenta solo **Editar** y **Quitar conexión**; **Conectar/Desconectar** permanece visible fuera del menú. Quitar solicita confirmación y nunca borra datos remotos.
4. Al abrirla y expandirla, se carga su lista de bases de datos. La primera carga puede iniciarse automáticamente al expandir.
4a. Bajo la conexión aparecen, además del grupo de bases de datos, los nodos **Procesos**, **Usuarios** y **Variables**. Cada sección se consulta solo al expandirla y tiene actualización independiente.
5. Al hacer clic en el nombre de una conexión desconectada, se abre y se carga su lista de bases; al hacer clic en el nombre de una base disponible, se carga o expande esa base. Si la conexión ya está abierta, la base se expande directamente. La flecha mantiene el mismo comportamiento de expansión.
6. Al expandir una base de datos, se cargan sus tablas y vistas. No se cargan los objetos de todas las bases de datos de antemano.
7. Al expandir una tabla o vista, se cargan sus columnas. No se cargan las columnas de todos los objetos de antemano.
8. Al hacer clic en el nombre de una tabla, se abre o activa su cuadrícula de datos de solo lectura según el spec 008. La activación no cambia ni reemplaza las demás pestañas.
9. La acción primaria de abrir datos no requiere un botón de ojo siempre visible en cada fila de tabla. Los controles persistentes del árbol se reservan para navegación y acciones frecuentes.
10. Un botón compacto de **Más opciones** (tres puntos) en la fila de tabla presenta las acciones secundarias, como **Ver estructura** y **Exportar**. **Ver estructura** abre la ficha de detalle del área central descrita arriba.
11. **Exportar** abre el selector de formatos compatibles con el objeto y el motor; después solicita las opciones propias del formato (por ejemplo, ruta, separador y marcador NULL para CSV). Los formatos no implementados no se anuncian como disponibles. El alcance funcional sigue el spec 004.
12. El menú contextual de clic derecho sigue disponible como vía adicional y ofrece las acciones de DBSUAL válidas para el objeto. Las acciones no aplicables o no disponibles por estado/permisos se ocultan o aparecen deshabilitadas con su causa.
13. La persona puede contraer y volver a expandir los nodos sin perder la orientación en el árbol.
14. **Actualizar** vuelve a consultar el nivel elegido. La aplicación muestra el resultado más reciente y conserva la selección cuando el objeto todavía existe.
15. Si una consulta falla, se muestra el error en el nivel afectado y se ofrece **Reintentar**. Los otros nodos utilizables permanecen visibles.
16. **Propiedades** abre una ficha con navegación por secciones para General, Información, Llaves, Campos, Llaves foráneas, Triggers, Particiones, Dependencias, Extras y Código fuente. Las secciones no compatibles con el objeto o motor se identifican como no disponibles y no como vacías.
17. La ficha diferencia valores descriptivos de opciones editables. Guardar una modificación de propiedades prepara una revisión; no escribe directamente en la base.

### Secciones de servidor

- **Procesos:** lista sesiones/conexiones que el motor permita inspeccionar, con identificador, usuario, host, base activa, comando/estado y duración cuando estén disponibles. El texto de consultas puede contener datos sensibles: se oculta o minimiza por defecto, no se guarda en historial ni logs y requiere una acción explícita para revelarse. Solo se muestran sesiones visibles según los permisos de la conexión.
- **Usuarios:** lista las cuentas que el motor permita ver, diferenciando cuenta/host y mostrando únicamente metadatos no secretos (por ejemplo, método de autenticación o privilegios visibles según permisos). Nunca consulta ni presenta hashes de contraseñas, secretos o credenciales.
- **Variables:** lista variables del servidor en lectura, distinguiendo alcance global y de sesión cuando aplique, con nombre, valor visible, alcance y descripción si está disponible. Los valores potencialmente sensibles se ocultan; la interfaz no los persiste.
- Estas secciones son de inspección en el alcance inicial. Terminar procesos, crear/editar/eliminar usuarios o cambiar variables son acciones administrativas distintas que deben declarar impacto y pasar por revisión, confirmación y aplicación cuando modifiquen el servidor. Los specs 006/015 exigen recuperación y comprobación de conflictos según el efecto; si no se puede cumplir, se bloquean.
- Los permisos insuficientes producen un estado de error o acceso restringido con opción de reintentar; nunca una lista vacía engañosa. Se informa cuando el motor solo permite mostrar una parte del catálogo.

### Vista detallada de estructura

- **Ver estructura** abre por defecto una pestaña de detalle en el área central de trabajo, no una modal pequeña que tape el explorador y las demás pestañas. Si el producto ofrece un modo de diálogo auxiliar, debe ser una opción explícita y conservar una distribución legible.
- La ficha de estructura identifica conexión, base de datos, esquema cuando corresponda, objeto y tipo (**tabla** o **vista**). La ruta completa del objeto se muestra como contexto y se trunca solo visualmente cuando no quepa, con acceso al nombre completo.
- Para tablas, muestra columnas ordenadas como en el motor: nombre, tipo, nulabilidad, valor predeterminado, generación/auto incremento cuando aplique y pertenencia/posición en claves.
- Muestra por secciones las claves primarias y únicas, índices, restricciones y claves foráneas; las relaciones indican columnas locales y objeto/columnas referenciados. No se infieren relaciones a partir de nombres.
- **Propiedades** amplía este detalle con las secciones pertinentes del motor y objeto: **General** (nombre, tipo de tabla/motor, charset, collation, comentario, formato de filas, autoincremento y opciones equivalentes), **Información** (estadísticas y metadatos disponibles), **Llaves** y **Campos**, además de **Llaves foráneas**, **Triggers**, **Particiones**, **Dependencias**, **Extras** y **Código fuente/DDL** cuando apliquen.
- Cada sección indica si los datos son de solo lectura, editables en un borrador o no compatibles. Los metadatos no proporcionados por el servidor se identifican; no se inventan valores.
- La intención de producto es que las propiedades administrables puedan gestionarse desde esta ficha. Las ediciones de nombre, motor/almacenamiento, charset/collation, comentario, formato de filas, autoincremento, llaves/campos y otras opciones preparan operaciones de esquema y pasan por el historial DBSUAL. No hay edición directa desde la ficha.
- Relaciones y objetos dependientes (claves foráneas, triggers, particiones y dependencias) deben mostrar el destino y el impacto conocido antes de confirmar una revisión. Si el impacto o la recuperación no se pueden determinar según los specs 010 y 015, la aplicación queda bloqueada.
- **Código fuente** muestra el DDL devuelto por el motor en modo de lectura y permite copiarlo. Modificar o aplicar ese SQL usa el flujo del editor y control de cambios de los specs 006 y 007.
- Las secciones tienen jerarquía visual y espaciado propios. Los datos tabulares se presentan en columnas alineadas con encabezados y filas separadas; no se concatenan etiquetas y valores en una línea ambigua. Los tipos largos pueden truncarse con su contenido íntegro accesible.
- La ficha usa el ancho disponible, permite desplazamiento vertical si el contenido excede el espacio, y mantiene encabezados/acciones identificables sin superponer contenido. Columnas, índices y restricciones se distinguen visualmente y muestran recuentos cuando estén disponibles.
- Cada sección tiene estados de carga, vacía, no disponible y error. Un error o falta de permisos no se presenta como ausencia de objetos.
- La acción **Actualizar estructura** vuelve a consultar el catálogo. Si el objeto desapareció, se informa y se permite cerrar la pestaña; no se presenta el detalle en caché como actual.
- Este alcance es de inspección. Crear, cambiar o eliminar estructura requiere el flujo de cambios de los specs 006 y 015.

## Reglas por motor

- **MySQL:** se muestran las bases de datos visibles para la cuenta conectada, con sus tablas, vistas y columnas accesibles.
- **MariaDB:** se muestran sus bases, tablas, vistas y columnas accesibles con un adaptador y pruebas propios, aunque comparta parte del protocolo con MySQL.
- **PostgreSQL:** se muestran las bases de datos accesibles de la instancia. Al expandir una base de datos se establece el contexto necesario para consultar sus objetos. Si existen varios esquemas, las tablas y vistas se agrupan por esquema para evitar nombres ambiguos.
- **SQLite:** cada conexión representa un archivo de base de datos. El explorador muestra ese archivo como nodo de base de datos y lista sus tablas, vistas y columnas al expandir los niveles correspondientes.
- **Procesos, Usuarios y Variables:** el contenido y permisos dependen del motor. MySQL/MariaDB exponen sesiones, cuentas y variables; PostgreSQL expone sesiones, roles y parámetros con sus propias reglas. SQLite no tiene sesiones remotas ni usuarios del motor; las secciones sin equivalente se marcan **No disponible para este motor**, no como error o lista vacía.

El explorador refleja lo que el motor permite ver a la cuenta; no presenta como inexistente un objeto cuyo listado falló por permisos o conectividad.

## Estados y casos límite

- **Cargando:** indicador junto al nodo consultado. Una acción repetida no dispara consultas duplicadas mientras la carga sigue activa.
- **Vacío:** mensaje específico para una conexión sin bases de datos visibles, una base de datos sin tablas o vistas visibles, o un objeto sin columnas visibles.
- **Error:** mensaje comprensible y acción de reintento. No se sustituye por una lista vacía.
- **Desconexión:** el árbol deja de ofrecer acciones que requieren la conexión. Al volver a conectar, se actualiza antes de presentar los datos como vigentes.
- **Objeto eliminado externamente:** al actualizar, desaparece del árbol; si estaba seleccionado, la selección se limpia con un aviso.
- Los nombres de bases de datos, esquemas, tablas, vistas y columnas se muestran tal como los devuelve el motor, sin usar el texto visible como identificador único interno.
- La clave primaria se indica en cada columna que la integra, incluidas las claves compuestas. Si el motor no ofrece ese dato para una vista, se muestra como no disponible en lugar de afirmar que la columna no es clave primaria.
- El valor predeterminado se muestra como expresión devuelta por el motor. Se diferencia entre «sin valor predeterminado» y «metadato no disponible»; la interfaz no evalúa ni modifica la expresión.
- La lista debe poder desplazarse y funcionar con teclado. Los nodos indican de forma accesible su estado expandido o contraído.

## Criterios de aceptación

1. Dada una conexión MySQL abierta con bases de datos visibles, al expandirla aparecen las bases de datos a las que la cuenta tiene acceso.
2. Dada una base de datos MySQL con tablas y vistas visibles, al expandirla aparecen en grupos diferenciados sin haber cargado antes los objetos de las demás bases de datos.
3. Dada una tabla o vista MySQL visible, al expandirla aparecen sus columnas con nombre, tipo de dato y nulabilidad, además de clave primaria y valor predeterminado cuando el motor entregue esos metadatos.
4. Dada una tabla con clave primaria compuesta, cada columna que pertenece a la clave aparece identificada.
5. Dada una vista cuyos metadatos no incluyen clave primaria o valor predeterminado, la interfaz indica que esos datos no están disponibles sin inventar un valor.
6. Dada una conexión, base de datos u objeto sin elementos visibles, se muestra un estado vacío específico para ese nivel.
7. Dado un error de permisos o de red durante un listado, se muestra un error y **Reintentar**; no se indica que el nivel esté vacío.
8. Dada una tabla, vista o columna creada, cambiada o eliminada fuera de DBSUAL, **Actualizar** refleja el cambio y conserva la selección solo si el objeto sigue existiendo.
9. Dada una desconexión mientras se carga un nivel, el resultado tardío no se presenta como información vigente de una conexión activa.
10. La navegación por teclado permite expandir, contraer y seleccionar nodos sin usar el ratón.
11. **Ver estructura** abre una ficha amplia en el área central de trabajo y permite mantenerla junto a otras pestañas; no presenta el detalle comprimido en una modal pequeña por defecto.
12. Columnas, índices y restricciones se muestran en secciones diferenciadas; cada atributo tiene una columna/etiqueta alineada y cada fila se distingue visualmente de la siguiente.
13. En una ventana de tamaño habitual de Windows, la ficha aprovecha el espacio disponible, permite desplazarse por contenido largo y no solapa encabezados, datos ni acciones.
14. Un clic en el nombre de una tabla abre la pestaña de datos de solo lectura; abrir otras tablas mantiene sus pestañas independientes.
15. Un clic en el nombre de una conexión desconectada inicia la conexión y carga las bases; un clic en el nombre de una base de datos expande/carga sus objetos cuando está disponible. Los errores de conexión o carga se muestran y no se presentan como contenido vacío.
16. Las filas del árbol no necesitan mostrar permanentemente los botones de ojo, estructura y exportación; **Más opciones** agrupa estructura y exportación, y el selector solo ofrece formatos realmente disponibles.
17. Varias conexiones guardadas muestran acciones independientes: **Conectar**/**Desconectar** visible y **Editar**/**Quitar conexión** dentro de **Más opciones**.
18. **Más opciones** de una tabla ofrece **Propiedades**. La ficha presenta las secciones previstas para el motor y el objeto y separa los metadatos no disponibles de las secciones vacías.
19. Las propiedades editables se pueden guardar como borrador/revisión, pero no se aplican al confirmar; su aplicación requiere las comprobaciones de conflicto y recuperación.
20. En una conexión MySQL, **Procesos**, **Usuarios** y **Variables** aparecen aparte de las bases. Expandir uno carga su contenido sin bloquear ni sustituir el árbol de bases de datos.
21. Con una cuenta sin privilegios suficientes, la sección correspondiente informa acceso restringido/error; no afirma que no haya elementos.
22. La sección Usuarios nunca expone hashes de contraseña y Procesos no revela texto de consultas sensibles por defecto.

## Estado actual de implementación

- PostgreSQL: el árbol lista tablas y vistas agrupadas por esquema y columnas con tipo, nulabilidad, clave primaria y valor predeterminado. Abre un contexto por base, conserva nombres homónimos en esquemas distintos y muestra errores de catálogo sin convertirlos en listas vacías. La integración PostgreSQL 16 verificó dos esquemas con tablas homónimas, una vista y metadatos de columnas. La ventana nativa y una cuenta de permisos mínimos todavía requieren validación.
- Implementado parcialmente para MySQL: el árbol carga bases, tablas, vistas y columnas bajo demanda. Las columnas muestran tipo, nulabilidad, pertenencia a clave primaria y valor predeterminado cuando están disponibles.
- Parcial para MySQL: **Ver datos** abre una pestaña de cuadrícula independiente y permite cargar páginas siguientes; Playwright verifica la pestaña y el error explícito cuando el core no está disponible. La ruta de lectura/paginación del core se probó contra MySQL 8.4, pero falta comprobar el recorrido completo desde la cuadrícula nativa.
- **Ver estructura** abre una ficha de solo lectura para tablas MySQL con columnas, índices, restricciones y referencias de claves foráneas. La ruta del core se probó contra MySQL 8.4 con claves compuestas y referencias compuestas. Columnas y catálogo de índices/restricciones conservan sus errores por separado; si una consulta falla, las secciones que sí cargaron permanecen visibles. Índices y restricciones todavía comparten una consulta de catálogo. No cubre aún atributos de generación/auto incremento, posición PK por columna, vistas, permisos mínimos ni otros motores.
- La ficha de estructura ahora se abre en una pestaña del área central, conserva la pestaña anterior y puede actualizar sus metadatos. La vista separa columnas, índices y restricciones en una tabla amplia y tarjetas alineadas; identifica conexión, base, motor y tabla. La prueba Playwright con IPC simulado comprueba columnas, clave primaria, referencia foránea y cambio de pestaña. Falta revisar la legibilidad en la ventana Tauri nativa y completar los atributos de columnas, vistas, permisos mínimos y otras áreas de Propiedades.
- **Verificación de ficha de estructura (2026-09-30):** `tests/e2e/table-structure.spec.ts` pasa en preview con IPC simulado (1/1): abre desde Más opciones, muestra metadatos tabulares y tarjetas, permite volver a Bienvenida y reactivar/cerrar la ficha. `npm run build` y la suite Playwright completa pasaron (26/26). Esto no sustituye revisar disposición y contraste desde la ventana Tauri nativa ni una conexión MySQL real.
- La imagen de referencia ilustra una cuadrícula para explorar filas; es referencia de experiencia, no especifica por sí sola paginación, permisos ni edición.
- En la interfaz MySQL/MariaDB, las filas de tablas ya no muestran botones persistentes de Ver datos, Ver estructura ni Exportar CSV: un clic en el nombre abre datos y el botón de tres puntos agrupa Ver estructura y Exportar como CSV. El menú contextual de clic derecho también conserva las acciones aplicables.
- **Propiedades** debe ofrecer las áreas mostradas en la referencia: llaves foráneas, triggers, particiones, dependencias, extras, código fuente, general, información, llaves y campos. En MySQL la ficha actual cubre solo columnas, índices y restricciones; faltan la interfaz completa y los adaptadores para las demás áreas.
- El árbol MySQL incluye **Procesos**, **Usuarios** y **Variables** como nodos independientes, cargados al expandirse y con actualización separada. Procesos no solicita texto SQL; Usuarios consulta cuentas visibles sin seleccionar hashes y muestra errores de permisos; Variables consulta el alcance global y redacta nombres con marcadores de secreto. La integración MySQL Community 8.4.11 comprobó las consultas con una cuenta `root`. La integración MariaDB verificó catálogo, columnas, estructura de tablas, procesos, cuentas y variables en 10.6.28, 10.11.19 y 11.4.13. En MariaDB el estado de bloqueo de cuentas se presenta como no disponible porque `mysql.user` no ofrece `account_locked` en todas las versiones admitidas. Falta validar UI nativa, permisos mínimos, variables sensibles y TLS/SSH reales.
- Clic en el nombre implementado en MySQL/MariaDB: sobre una conexión desconectada o con error abre y expande el árbol; sobre una base de datos carga/expande sus objetos; sobre una tabla abre o activa su pestaña de datos. Falta verificar estos recorridos en la ventana nativa y contra una conexión real.

## Fuera de este spec

- Ver, filtrar, ordenar o editar filas; el comportamiento de la cuadrícula está en el spec 008.
- Crear, renombrar o eliminar tablas; las propiedades que impliquen estos cambios se gestionan mediante los specs 006 y 015.
- Buscar objetos entre conexiones.
- Ejecutar consultas SQL desde el explorador.

## Decisiones pendientes

1. Si el nombre de cada base de datos debe incluir información adicional, como tamaño o propietario.
2. Estados independientes por sección de la ficha y detalles de generación/auto incremento; se mantienen los datos de inspección descritos arriba como alcance funcional.

## Avance de exploración SQLite — 2026-09-30

La conexión local de solo lectura, catálogo de tablas/vistas y columnas de `main` y el acceso a filas desde la cuadrícula ya están implementados. Las bases adjuntas no se exploran todavía; la creación, eliminación y gestión de archivos se mantienen bloqueadas hasta validar el proveedor protegido del spec 013. Falta comprobar el recorrido visual de Tauri en Windows.
