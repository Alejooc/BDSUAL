# Spec 005 — Interfaz de trabajo de escritorio

Estado: borrador para revisión  
Versión: 1.0
Última actualización: 2026-09-30
Depende de: [001 — Conexiones](001-conexiones.md), [002 — Explorador](002-explorador.md), [006 — Control de cambios](006-control-de-cambios.md)

## Objetivo

Ofrecer en Windows, macOS y Linux un espacio de trabajo familiar para personas que usan editores como Visual Studio Code: navegación por conexiones y objetos a la izquierda, trabajo en pestañas en el centro y estado de operaciones siempre visible. La interfaz debe dejar claro el destino y el efecto de cada acción antes de modificar una base de datos. Los mínimos de ventana y particularidades de WebView se verifican por plataforma según el spec 019.

## Estructura inicial

| Zona | Contenido y función |
| --- | --- |
| Barra superior | Identidad, estado de DBSUAL, acceso para abrir de nuevo **Inicio/Bienvenida** y controles de ventana. |
| Barra de actividad | Cambia entre Explorador, Cambios e Historial. En su zona inferior ofrece Configuración mediante un icono, además de acciones de ayuda y disposición. Indica cuando hay cambios preparados o una operación que requiere atención. |
| Panel lateral | Muestra el contenido de la sección elegida. En Explorador contiene conexiones, bases de datos, esquemas cuando corresponda, tablas, vistas y columnas. |
| Área central | Pestañas para editor SQL, cuadrícula de datos, estructura de tablas, detalles de conexión y objetos, vistas previas, importación/exportación y revisión de cambios. |
| Resultados SQL | Aparecen junto al editor de la pestaña Consulta SQL. No hay un panel inferior persistente sin contenido. |
| Barra de estado | Motor, conexión, base de datos y proyecto activos; estado de conexión y de la última operación. |

El panel lateral y el inferior se pueden redimensionar, contraer y restaurar. La aplicación recuerda sus tamaños y la sección activa entre sesiones, sin impedir volver a una disposición predeterminada.

## Tipografía y legibilidad

- La interfaz usa una escala tipográfica consistente: texto principal de 15–16 px, etiquetas y controles de 14 px, datos tabulares densos de al menos 13 px y metadatos secundarios de al menos 12 px. El código SQL conserva una escala legible independiente.
- Encabezados y títulos tienen jerarquía por tamaño y peso, no solo por color o mayúsculas. Los textos de ayuda y errores usan interlineado suficiente y contraste legible.
- Los controles de texto y elementos del árbol tienen altura/padding que admite la tipografía elegida; las etiquetas no se recortan ni se superponen al aumentar el tamaño.
- La escala se mantiene coherente en explorador, conexiones, cuadrículas, estructura, menús, diálogos, historial y barra de estado. Los datos largos pueden truncarse visualmente, pero siguen disponibles completos mediante tooltip/selección/scroll.
- La tipografía prioriza legibilidad en la ventana mínima admitida de cada plataforma y no reduce información esencial a texto diminuto para ahorrar espacio.

## Contexto y navegación

- Se pueden guardar y abrir varias conexiones. Cada pestaña muestra de forma visible a qué conexión y base de datos pertenece.
- Se pueden mantener abiertas varias pestañas de cuadrícula de datos, cada una ligada a una tabla y contexto propios. Abrir otra tabla no reemplaza la cuadrícula existente; volver a abrir una tabla con la misma identidad activa su pestaña.
- La pestaña activa usa un único indicador de selección: una sola línea de acento en su borde superior, sin borde duplicado entre el contenedor y el botón ni una segunda barra superpuesta.
- **Ver estructura** abre por defecto una pestaña de detalle en el área central. La modal compacta no es la presentación predeterminada para explorar columnas, índices y restricciones.
- Cambiar la selección en el explorador no cambia silenciosamente el destino de una pestaña ya abierta.
- Si una conexión se desconecta o una base de datos deja de existir, las pestañas dependientes muestran su estado y desactivan acciones que requieren ese destino.
- La barra de estado refleja el contexto de la pestaña activa. Si no hay pestaña activa, no sugiere un destino de operación inexistente.
- Los nombres largos se pueden consultar completos. Objetos con el mismo nombre en distintas conexiones o esquemas se distinguen por su ruta.
- La interfaz no usa solo color o iconos para comunicar conexión, error, cambio preparado o acción destructiva.
- **Inicio/Bienvenida** es una pestaña de trabajo normal: puede cerrarse como cualquier pestaña y no vuelve a abrirse automáticamente por ocupar el último espacio. La persona puede abrirla desde la barra superior cuando la necesite.
- **Configuración** se abre desde un icono sin texto en la parte inferior de la barra de actividad. Presenta una página completa en el área central, no una modal; conserva las demás pestañas.
- La página organiza sus opciones en categorías navegables: Apariencia permite elegir temas completos, personalizar colores y ajustar la escala tipográfica según el spec 020; Editor permite cambiar el tamaño de fuente SQL; Espacio de trabajo administra la disposición; Seguridad y recuperación administra el depósito cifrado. Las opciones se aplican de inmediato, persisten entre reinicios y se validan en Rust.
- Si una operación necesita una configuración que falta, DBSUAL ofrece un acceso directo a la sección pertinente de Configuración. Al terminar, la persona puede volver a la operación pendiente sin perder su contexto.
- En el árbol, un clic en el nombre ejecuta la acción primaria del nodo: abre datos para una tabla; expande una conexión o base de datos y conecta cuando haga falta. La flecha conserva la acción de expansión. Un menú de tres puntos agrupa las acciones secundarias específicas del objeto y evita llenar cada fila con botones persistentes.
- En cada fila de conexión, **Conectar**/**Desconectar** permanece como acción primaria; **Editar** y **Quitar** están dentro de **Más opciones**. Los estados y acciones de varias conexiones no se mezclan.

## Menús contextuales

Implementación actual (parcial): DBSUAL suprime el menú genérico del WebView y presenta menús propios para conexiones, bases de datos, tablas, vistas, pestañas y cuadrículas de datos. Los menús permiten navegación con flechas y Escape. En el explorador, un clic en el nombre de una conexión la abre y expande si está desconectada; en una base de datos carga/expande sus objetos; en una tabla abre o activa su pestaña de datos. La flecha conserva la expansión. Las filas de tablas MySQL/MariaDB ya agrupan estructura y exportación en un botón de tres puntos. Falta verificar estos recorridos en la ventana nativa. Faltan acciones contextuales específicas para columnas, editor SQL y campos de edición; esos criterios siguen pendientes.

- Dentro de la ventana de DBSUAL, el clic derecho no muestra el menú contextual genérico del navegador/WebView.
- En un objeto o zona donde existan acciones de DBSUAL, el clic derecho abre un menú contextual propio, asociado al contexto exacto: conexión, base de datos, esquema, tabla, vista, columna, pestaña, cuadrícula, editor SQL o espacio vacío.
- El menú incluye únicamente acciones válidas para el elemento y su estado actual. Por ejemplo, una tabla puede ofrecer **Ver datos**, **Ver estructura**, **Actualizar** y exportación disponible; una vista no ofrece acciones propias de escritura de tabla.
- Acciones que cambien una base de datos conducen al flujo de preparar, revisar, confirmar y aplicar, con las confirmaciones y recuperación exigidas por los specs correspondientes. Un menú contextual no omite esas puertas.
- En campos de texto y editores, se mantienen las acciones de edición propias de DBSUAL que correspondan, sin exponer el menú del navegador. En zonas sin acciones aplicables, el clic derecho no abre un menú ajeno a la aplicación.
- En la cuadrícula, el menú contextual de una celda permite copiar su valor; cuando el clic se hace sobre un editor inline activo, ofrece las acciones de texto disponibles, incluida Pegar. El alcance y las reglas de que pegar solo propone un valor están definidos en el spec 008.
- El menú se posiciona junto al puntero, permanece dentro del área visible de la ventana y se cierra al elegir una acción, hacer clic fuera o pulsar Escape. Sus acciones también están disponibles por teclado y se anuncian con etiquetas accesibles.

## Cambios e historial

- **Cambios** separa borradores, revisiones confirmadas sin aplicar y operaciones en curso. Cada elemento muestra destino, resumen e impacto conocido.
- Toda acción que modifica una base ofrece **Preparar cambio** y conduce a una vista previa; no se ejecuta en la base al preparar o confirmar la revisión.
- **Historial** muestra revisiones, aplicaciones y restauraciones en orden temporal, con su estado y punto de recuperación cuando exista.
- La interfaz muestra a qué historial local pertenece el destino. Si no puede abrir o guardar ese historial, bloquea las acciones que modifican la base y explica el problema.
- **Aplicar a la base** identifica explícitamente conexión, base de datos y revisión; los efectos y la disponibilidad de recuperación se consultan antes de confirmar.
- Una operación con estado incierto permanece destacada hasta que se vuelva a inspeccionar el destino. No se presenta como éxito ni como fallo total sin evidencia.

## Estados de la aplicación

- **Inicio sin conexiones:** explica cómo crear la primera conexión y ofrece la acción correspondiente.
- **Conexión desconectada:** mantiene visible la configuración guardada y ofrece conectar o editar.
- **Carga:** muestra el nodo o la operación afectada sin bloquear el resto de la interfaz cuando no sea necesario.
- **Vacío:** diferencia entre ausencia real de objetos y falta de selección.
- **Error:** describe el problema en el lugar afectado y ofrece la siguiente acción útil, como reintentar, editar conexión o actualizar.
- **Operación prolongada:** muestra etapa, avance cuando se pueda medir y opción de cancelar solo cuando la cancelación sea segura o se explique su alcance.

## Persistencia y cierre

- Al cerrar y abrir la aplicación, se restauran disposición, pestañas y selección que sigan siendo válidas. Las conexiones no se abren automáticamente sin una decisión expresa de la persona.
- Bienvenida se puede cerrar. Si no quedan pestañas de trabajo abiertas, el área central muestra un estado vacío con acciones para abrir **Inicio**, crear una pestaña SQL o volver al explorador; no reabre Bienvenida forzosamente.
- La sesión restaura solo pestañas y selección válidas que se puedan persistir de forma segura. No conserva texto SQL, resultados ni destinos sensibles; las pestañas restauradas no inician conexiones automáticamente.
- **Configuración** está disponible desde el icono inferior de la barra de actividad aunque Bienvenida esté cerrada. Ofrece una página completa para administrar apariencia, tipografía, editor SQL, disposición y depósito de recuperación.
- Los borradores de cambios y formularios de importación o exportación no se descartan silenciosamente. Si hay contenido sin guardar, el cierre lo conserva o solicita una decisión clara.
- El historial y los puntos de recuperación pertenecen al proyecto; cerrar una pestaña o una ventana no los borra.

## Accesibilidad y uso con teclado

- Las zonas principales, pestañas, árbol y acciones son navegables con teclado y muestran el foco.
- Los paneles redimensionables también se pueden ajustar sin ratón.
- Los mensajes de estado importantes se anuncian a tecnologías de asistencia sin interrumpir repetidamente a la persona.
- Los diálogos de confirmación mantienen el foco dentro del diálogo hasta cerrarse y devuelven el foco a la acción de origen.

## Criterios de aceptación

1. Con dos conexiones que contienen bases de datos del mismo nombre, cada pestaña y acción muestra el destino completo y no cambia de destino al seleccionar otro nodo.
2. Tras redimensionar paneles y reiniciar, la disposición se restaura y existe una acción para volver a la disposición predeterminada.
3. Al preparar una eliminación, el panel **Cambios** muestra el borrador y la base de datos permanece intacta hasta elegir **Aplicar a la base**.
4. Al abrir **Historial**, se distinguen revisiones confirmadas, aplicaciones exitosas, fallidas e inciertas, y restauraciones.
5. Si una conexión falla, el explorador y las pestañas afectadas muestran el error y una acción útil; otras conexiones siguen siendo utilizables.
6. Si se cierra la aplicación con un borrador, al volver a abrirlo se puede recuperar o se había pedido una decisión explícita antes de salir.
7. Una persona puede recorrer las zonas principales, expandir objetos y activar acciones con teclado sin depender del ratón.
8. Una pestaña de editor SQL muestra su conexión y base de datos de destino; sus resultados aparecen en la misma vista del editor y no se mezclan con los de otra pestaña.
9. Una pestaña de cuadrícula muestra su tabla de destino y diferencia los valores vigentes de las ediciones preparadas que aún no se han aplicado.
10. Al hacer clic derecho en una tabla, conexión o pestaña, aparece un menú de DBSUAL con acciones válidas para ese elemento; no aparece el menú genérico del navegador.
11. Al hacer clic derecho en un área sin acciones contextuales o en un campo de edición, no aparece un menú del navegador; las acciones propias de edición que DBSUAL admita siguen disponibles.
12. Al abrir el menú contextual con teclado, sus opciones son accesibles, respetan el estado del objeto y cualquier acción modificadora conserva el flujo protegido.
13. Al abrir dos tablas desde el explorador, hay dos pestañas de cuadrícula; cambiar entre ellas conserva el contenido y destino de cada una. Reabrir la primera activa su pestaña existente.
14. **Ver estructura** abre el detalle en el área central con secciones legibles, filas separadas y columnas alineadas; se puede usar junto a las cuadrículas y editor sin taparlos.
15. Clic en el nombre de una tabla abre su cuadrícula; no es necesario un botón de ojo en cada fila.
16. **Más opciones** de una tabla incluye **Ver estructura** y **Exportar**. Exportar permite escoger un formato disponible y luego sus opciones; los formatos no implementados no aparecen habilitados.
17. Clic en el nombre de una conexión desconectada abre la conexión y carga sus bases; clic en el nombre de una base disponible carga/expande esa base sin abrir una conexión duplicada.
18. Con varias conexiones guardadas, cada una muestra **Conectar** o **Desconectar** según su estado y su propio menú **Más opciones** con **Editar** y **Quitar**; cambiar una conexión no altera visualmente las demás.
19. La pestaña Bienvenida tiene botón de cierre. Tras cerrarla no reaparece automáticamente; la persona puede abrirla desde la barra superior.
20. El icono Configuración aparece en la zona inferior de la barra de actividad, sin texto, y abre una página completa dentro del espacio de trabajo, sin modal.
21. Tema, escala de texto y tamaño de fuente SQL se pueden cambiar desde Configuración, se aplican de inmediato y persisten.
22. Las categorías Espacio de trabajo y Seguridad y recuperación permiten administrar la disposición y el depósito cifrado.
23. Si una operación bloqueada requiere configurar o importar una clave, ofrece navegar a la sección correspondiente y después permite retomar el flujo.
24. Al cerrar la última pestaña, el área central ofrece un estado vacío útil y no crea una pestaña Bienvenida implícita.
25. Al seleccionar una pestaña aparece un único acento superior continuo; no se ven dos líneas paralelas ni bordes duplicados.
26. En las superficies de uso habitual, los tamaños respetan la escala tipográfica establecida y los controles mantienen altura suficiente para sus etiquetas.
27. En la ventana mínima acordada para cada target (nunca menor que 720 × 520 px), el explorador, formularios y cuadrícula siguen siendo legibles y utilizables, con scroll donde sea necesario y sin texto cortado; el recorrido se comprueba en el WebView nativo de cada plataforma declarada compatible.

## Fuera de este spec

- Reglas de sintaxis y ejecución de consultas SQL, definidas en `007-editor-sql.md`.
- Reglas de lectura y edición de filas, definidas en `008-cuadricula-datos.md`.
- Edición de filas y estructuras.
- Iconografía detallada y acabado visual definitivo.
- Diseño de pantallas de administración específicas de cada motor.

## Decisiones pendientes

1. Si las conexiones guardadas deben abrirse automáticamente al restaurar una sesión, como opción configurable.
2. Qué pestañas de detalle se necesitan al seleccionar una tabla o vista, además de sus columnas en el explorador.
3. Inventario visual completo y atajos de las acciones contextuales por tipo de elemento; el comportamiento obligatorio del clic derecho y la supresión del menú del navegador quedan definidos arriba.
