# Spec 017 — Base de la aplicación Windows

Estado: implementada; pendiente de revisión del producto  
Versión: 0.4
Última actualización: 2026-09-30
Depende de: [005 — Interfaz](005-interfaz-de-trabajo.md), [009 — Arquitectura](009-arquitectura.md), [016 — Plan SDD](016-plan-sdd-mvp.md)

## Objetivo

Definir la entrega 0 de Windows: una aplicación Tauri 2 que abre en Windows, presenta el espacio de trabajo inicial, guarda preferencias no sensibles y establece un contrato estable entre React y Rust. Esta ficha conserva el alcance y la evidencia específica de Windows; el objetivo multiplataforma y sus puertas se definen en el spec 019. Todavía no necesita conectarse a un motor de base de datos.

## Estructura visible

- La ventana muestra barra de actividad, panel lateral, área central y barra de estado según el spec 005. No reserva espacio para un panel inferior de salida vacío.
- La barra superior ofrece acceso para abrir **Inicio/Bienvenida**. Un icono sin texto en la parte inferior de la barra de actividad abre la página completa de **Configuración** dentro del área central, no una modal.
- **Explorador**, **Cambios** e **Historial** se pueden seleccionar. Cuando no hay conexiones o revisiones, muestran estados vacíos claros y una acción pertinente; no simulan objetos de una base.
- Los paneles lateral e inferior se redimensionan, contraen y restauran con ratón y teclado. La acción **Restablecer disposición** vuelve a los tamaños iniciales.
- Bienvenida es una pestaña normal que la persona puede cerrar; al cerrar la última pestaña, el centro muestra un estado vacío con accesos para abrir Inicio, crear una pestaña SQL o ir al Explorador.
- La administración del depósito de recuperación se abre desde **Configuración**, no forma parte del contenido de Bienvenida. Las operaciones que necesiten una clave enlazan directamente con esa configuración.
- Una pestaña SQL vacía puede restaurarse sin texto ni destino y no crea una conexión activa. Bienvenida no se agrega automáticamente si se cerró.
- Los mensajes de carga y error aparecen en la zona afectada; un error al guardar una preferencia no cierra ni congela la ventana.

## Almacenamiento local

- Rust resuelve el directorio de datos de la aplicación mediante la API de rutas de Tauri y crea allí el SQLite interno. No se construye una ruta fija a partir del nombre de usuario Windows.
- La base local empieza con tablas o estructuras versionadas para **migraciones**, **preferencias de interfaz** y **sesión restaurable**. Las entidades de conexiones, proyectos, revisiones, trabajos y artefactos se agregan mediante migraciones en las entregas que las utilicen.
- Las preferencias incluyen sección activa, tamaño y estado de paneles, tema, escala tipográfica, tamaño de fuente SQL y versión del formato. Rust valida rangos y valores permitidos; campos añadidos admiten valores predeterminados al leer preferencias anteriores.
- La sesión restaurable contiene solo identificadores y contexto no secreto, como tipo y orden de pestañas. No se guardan contraseñas, frases de recuperación, claves, resultados de consultas ni texto SQL con datos en claro.
- Si una pestaña restaurada apunta a una conexión o base que ya no existe, se muestra como no disponible o se omite con aviso; nunca se reconecta automáticamente.
- El archivo SQLite y sus migraciones pertenecen al core. Zustand mantiene el estado visual en memoria, pero no escribe directamente el archivo.

## Arranque y migraciones

1. Tauri inicia el core y resuelve el directorio local. Comprueba acceso de lectura y escritura antes de declarar el almacenamiento disponible.
2. El core abre el SQLite interno y aplica migraciones pendientes en orden. Cada migración termina con versión registrada solo después de confirmar su transacción.
3. Lee preferencias y sesión válidas, entrega un resumen de arranque al frontend y cambia las revisiones de historial que quedaron en `applying` a `uncertain`, con un evento durable. Nunca reejecuta automáticamente una operación remota interrumpida.
4. React representa la disposición restaurada o una disposición inicial segura. No abre conexiones por iniciativa propia.

Si la base local está dañada, tiene una versión de esquema más nueva que la aplicación o no puede escribirse, DBSUAL informa la causa y evita crear un historial alternativo vacío sobre el archivo existente. Las modificaciones de bases externas permanecerán bloqueadas hasta resolverlo; el tratamiento de lectura degradada se concretará al incorporar conexiones.

## Contrato IPC inicial

| Comando | Entrada | Salida observable |
| --- | --- | --- |
| `bootstrap` | Ninguna. | Versión de contrato, estado del almacenamiento, preferencias y sesión restaurable. |
| `save_ui_preferences` | Preferencias tipadas con versión. | Preferencias confirmadas o error recuperable; nunca se anuncia guardado antes de persistir. |
| `save_session` | Lista validada de pestañas y sección activa, sin contenido sensible. | Sesión confirmada o error recuperable. |
| `reset_ui_preferences` | Ninguna. | Valores iniciales confirmados y aplicados a la interfaz. |

- La frontera usa tipos Serde en Rust y tipos TypeScript equivalentes. Los nombres y versiones del contrato se mantienen explícitos; una respuesta incompatible produce un error visible y no se interpreta como éxito.
- Los errores tienen código estable, mensaje breve para la persona y detalle técnico sin secretos. El frontend no depende de comparar textos de error para decidir el flujo.
- El core valida todo dato recibido: versión, tamaños de panel, identificadores, longitud y tipo de campos. No acepta rutas arbitrarias ni SQL en estos comandos.
- Los comandos disponibles se limitan mediante las capacidades y permisos de Tauri. La interfaz no recibe acceso general al sistema de archivos para leer el SQLite interno.
- Los trabajos largos futuros utilizarán identificador de trabajo y canal de progreso; la entrega 0 define el tipo común de estado sin crear trabajos ficticios.

## Cierre y recuperación de interfaz

- Al cerrar normalmente, se guardan preferencias y sesión no sensible. Si ese guardado falla, se informa y se ofrece reintentar o cerrar sabiendo qué no quedará restaurado.
- La sesión registra explícitamente si Bienvenida está abierta y su posición; la ausencia de esa pestaña se conserva tras reiniciar. La persona puede volver a abrirla desde la barra superior.
- El cierre de la ventana no elimina historial ni artefactos. En el siguiente arranque, las revisiones aún `applying` quedan `uncertain` con un evento; no hay reconciliación automática contra el estado del servidor ni repetición de SQL.
- Al consultar cambios o historial, ese estado se presenta en español con un aviso que indica inspeccionar la base remota antes de cualquier nueva revisión. Playwright valida la presentación con IPC simulado y que abrir planes no invoque comandos de aplicación; no valida el resultado remoto ni una reconciliación nativa.
- Un borrador SQL o un formulario con datos sin guardar no se persiste en claro. Hasta que exista el almacenamiento cifrado del spec 014, cerrar esa pestaña o la aplicación requiere una decisión explícita de descartar o permanecer. Una caída inesperada antes de disponer de cifrado puede perder ese borrador; la interfaz no promete lo contrario.
- Una sesión que se restauró correctamente conserva disposición y pestañas válidas, pero todas las conexiones aparecen desconectadas.

## Pruebas de aceptación de la entrega 0

1. La aplicación abre en Windows sin conexiones, muestra las cinco zonas y permite alternar Explorador, Cambios e Historial con estados vacíos comprensibles.
2. Tras cambiar tamaños y sección activa, cerrar y reabrir restaura los valores. **Restablecer disposición** recupera los tamaños iniciales.
3. Un tamaño guardado fuera de rango se corrige al arrancar y no deja invisible toda el área central.
4. Un fallo al guardar preferencias muestra error y no afirma que se guardaron; la interfaz sigue utilizable.
5. Una migración interrumpida o fallida no incrementa la versión del esquema ni sustituye el historial existente por una base vacía.
6. Una base local con esquema más nuevo que el ejecutable se detecta y se explica sin intentar degradarla automáticamente.
7. La inspección del SQLite interno confirma que no se guardan secretos, texto SQL ni resultados de consulta en las preferencias o sesión.
8. Los comandos rechazan tamaños inválidos, versiones incompatibles y campos inesperados según el contrato sin cerrar la ventana.
9. Una pestaña no sensible se restaura sin abrir su conexión automáticamente.
10. Las zonas principales y el cambio de tamaño de paneles se pueden usar con teclado y muestran el foco.
11. Bienvenida se puede cerrar y permanece cerrada tras reiniciar; se puede volver a abrir desde la barra superior.
12. El icono inferior de Configuración abre su página completa sin requerir Bienvenida abierta y muestra las categorías Apariencia, Editor, Espacio de trabajo y Seguridad y recuperación.
13. Tema, escala tipográfica y tamaño de fuente SQL se cambian desde Configuración, surten efecto en su superficie y sobreviven al reinicio.

## Decisiones de implementación

1. El identificador Tauri es `com.dbsual.desktop`; el core resuelve su directorio de datos mediante Tauri.
2. Node 24.13.0 y Rust 1.98.0 están fijados en el repositorio. Las dependencias JavaScript y Rust están bloqueadas en sus respectivos archivos de lock.
3. El SQLite local usa `PRAGMA user_version` y aplica la primera migración en una transacción. Las migraciones posteriores que transformen datos requerirán una estrategia de respaldo antes de implementarse.
4. El contrato IPC tiene tipos correspondientes en Rust y TypeScript. La generación automática queda pendiente para la siguiente ampliación del contrato.

## Evidencia de la entrega 0

- La aplicación se compiló y abrió como ventana nativa de Windows; al cerrarla guardó preferencias y sesión en el SQLite local.
- Las cuatro pruebas del core cubren validación, persistencia tras reapertura y rechazo de una versión de esquema más nueva.
- Las pruebas de interfaz cubren estados vacíos, cambio de sección y redimensionado con ratón y teclado.
- La sesión permite mantener Bienvenida, Consulta SQL o ninguna de ellas; no persiste texto SQL, resultados, destinos ni conexiones activas. La prueba Rust valida sesión vacía y Playwright comprueba cerrar Bienvenida, abrir Inicio desde la barra superior, cerrar la última pestaña y crear/cerrar SQL.
- Configuración abre una página completa desde el icono inferior de actividad; tema, escala tipográfica y tamaño de fuente SQL se guardan en preferencias versionadas. Falta comprobar en Windows que el reinicio restaure estas opciones y que los temas sean legibles a distintos tamaños.

## Empaquetado Windows

- CI construye los bundles MSI y NSIS y ejecuta `scripts/verify-windows-installers.ps1` antes de publicarlos como artefactos. La verificación exige exactamente un paquete de cada tipo, tamaño no vacío, nombre x64, metadatos de producto/versión coherentes con `tauri.conf.json`, `ProductCode`/`UpgradeCode` MSI válidos y plataforma x64 declarada por MSI. Para NSIS valida metadata del ejecutable y estructura PE; el bootstrapper NSIS puede ser x86 aunque el paquete Tauri se genere para x64.
- En una instalación temporal desechable Windows, el MSI terminó con código 0, dejó `dbsual.exe` en `INSTALLDIR` y se desinstaló con `msiexec /x`; se comprobó que ya no quedaban ni la entrada del producto ni el ejecutable. NSIS también terminó con código 0 usando `/S` y `/D`, creó el ejecutable y su desinstalador silencioso lo retiró.
- Estas pruebas cubren instalar/desinstalar los dos paquetes en esta máquina, sin iniciar la aplicación. No prueban Windows limpio, arranque desde instalación, actualización sobre una versión previa, conservación del historial/preferencias ni política de limpieza de datos. Los artefactos CI tampoco están firmados para publicación.
- Pendiente: recorrido en Windows limpio, actualización desde una versión anterior de prueba, arranque del ejecutable instalado, y anotar qué datos locales quedan tras desinstalar para contrastarlo con la política de producto.

## Referencias técnicas

- [Tauri 2: comandos, estado y canales](https://v2.tauri.app/develop/calling-rust/).
- [Tauri 2: permisos y capacidades](https://v2.tauri.app/security/permissions/).
- [Tauri 2: API de rutas de aplicación](https://v2.tauri.app/reference/javascript/api/namespacepath/).

## Verificación nativa Windows — 2026-09-30

`npm run tauri build` compiló el ejecutable x64 y generó los paquetes MSI y NSIS desde el código actual, incluida la vista previa CSV. `scripts/verify-windows-installers.ps1` validó nombre, plataforma y metadatos de ambos paquetes. En una verificación separada, ambos instaladores se instalaron y desinstalaron en directorios temporales de esta máquina; no se abrió el ejecutable instalado. Computer Use no pudo inicializarse en esta sesión porque el helper de Windows no encontró su ruta de recursos, así que la inspección de la ventana sigue pendiente. También falta probar un Windows limpio, actualizar desde una versión anterior, arrancar la app instalada, comprobar la persistencia al cerrar/reabrir y documentar qué datos se conservan al desinstalar.
