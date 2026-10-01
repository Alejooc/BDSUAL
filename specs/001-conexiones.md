# Spec 001 — Conexiones a bases de datos

Estado: MySQL/MariaDB parcialmente implementados; conexión directa PostgreSQL y conexión a archivo SQLite implementadas parcialmente
Versión: 0.8
Última actualización: 2026-09-30
Depende de: [000 — Visión y alcance](000-vision-y-alcance.md)

## Objetivo

Permitir que una persona en Windows, macOS o Linux configure, pruebe, guarde y utilice conexiones a motores de bases de datos. MySQL es el primer motor que se implementará. El mismo flujo deberá extenderse después a MariaDB, PostgreSQL y SQLite; credenciales, selector de archivos, TLS y SSH respetan los adaptadores por plataforma del spec 019.

## Alcance de la primera entrega de este spec

- Crear una conexión MySQL con nombre visible, host, puerto, usuario y contraseña opcional.
- Configurar TLS para la conexión con MySQL y, cuando se necesite, un túnel SSH hacia el servidor.
- Probar la conexión antes o después de guardarla.
- Guardar los datos no secretos de la conexión localmente y la contraseña, si existe, en el almacén de credenciales del sistema.
- Mostrar las conexiones guardadas y su estado.
- Abrir, desconectar, editar y quitar una conexión guardada.
- Comunicar los errores de conexión de forma útil sin mostrar secretos.

Implementación actual: Windows ofrece conexiones MySQL/MariaDB y PostgreSQL, además de abrir archivos SQLite existentes en solo lectura. Persisten pendientes el recorrido nativo completo y las matrices de seguridad/plataforma de los criterios específicos; SQLite no ofrece todavía escritura protegida ni administración de archivos.

Conectar a un servidor MySQL no requiere elegir una base de datos predeterminada. La selección y el listado de bases de datos se especifican en `002-explorador.md`.

## Modelo visible para la persona usuaria

Cada conexión guardada tiene:

| Campo | Regla |
| --- | --- |
| Nombre | Obligatorio; identifica la conexión en la interfaz. |
| Motor | MySQL en la primera entrega; MariaDB, PostgreSQL y SQLite en entregas siguientes. |
| Host | Obligatorio para MySQL. |
| Puerto | Obligatorio para MySQL; valor inicial sugerido: 3306. |
| Tipo de conexión | Selector por conexión: **Directa**, **DLL** o **HTTP Tunnel**. Determina qué parámetros de transporte se muestran y cómo se establece la sesión. |
| Usuario | Obligatorio para MySQL. |
| Contraseña | Opcional; nunca se vuelve a mostrar en texto claro después de guardar. |
| TLS | Configuración opcional por conexión; cuando está activada, se verifica la identidad del servidor. |
| Túnel SSH | Configuración opcional por conexión; cuando está activada, la conexión MySQL se establece a través del túnel. |

El identificador interno de la conexión será estable aunque cambie el nombre visible. Dos conexiones pueden apuntar al mismo servidor, pero sus nombres visibles deben distinguirse para evitar confusión.

### Tipo de conexión

- El formulario de crear y editar conexión muestra el tipo seleccionado y conserva el valor en la configuración. Al editar, el selector inicia con el tipo guardado.
- **Directa:** conecta al host y puerto MySQL por TCP. TLS es opcional con las reglas de este spec. Un túnel SSH configurado puede usarse como transporte intermedio sin desactivar TLS; la interfaz indica que SSH es un túnel agregado a la conexión directa.
- **DLL:** conecta mediante una biblioteca cliente nativa compatible con el motor y la arquitectura de DBSUAL. Al seleccionarla, el formulario debe indicar/seleccionar la biblioteca y validar existencia, arquitectura, versión/compatibilidad y dependencias antes de guardar o probar. No se carga una DLL arbitraria silenciosamente. ABI, biblioteca admitida, distribución y política de validación quedan pendientes; hasta entonces la opción aparece como no disponible y no permite guardar una conexión que afirme usarla.
- **HTTP Tunnel:** conecta mediante un endpoint HTTP de túnel configurado por la persona; el formulario solicita la URL y parámetros propios del protocolo, además de credenciales si el endpoint las requiere. HTTPS debe verificar el certificado del endpoint. No se envía la contraseña MySQL a un endpoint no validado ni se rebaja TLS del servidor MySQL sin una decisión documentada. Protocolo compatible, autenticación, límites, errores y soporte de consultas/metadatos quedan pendientes.
- Al cambiar de tipo durante una edición, la aplicación conserva los valores de otros tipos en el formulario o advierte que se descartarán; nunca los reutiliza como parámetros del tipo nuevo.
- No se presenta éxito al probar un tipo que no esté implementado. El estado **No disponible** explica que la capacidad aún no está soportada.

### Acciones en la lista de conexiones

- La lista puede contener varias conexiones guardadas a distintos servidores, bases o perfiles. Cada fila muestra un nombre y estado que permiten distinguirlas; las acciones siempre afectan solo a la conexión de esa fila.
- **Conectar**/**Desconectar** es la acción primaria visible de la fila y cambia según el estado. Mientras conecta, se deshabilitan acciones incompatibles y se muestra el progreso.
- **Editar** y **Quitar conexión** son las acciones del botón **Más opciones** (tres puntos) en la fila. No ocupan botones persistentes y **Conectar/Desconectar** permanece fuera del menú como acción primaria visible.
- El menú identifica la conexión a la que aplica y cierra tras ejecutar una acción. **Quitar conexión** solicita confirmación y aclara que elimina la configuración/credencial local, no la base de datos del servidor.
- El menú contextual de clic derecho definido en el spec 005 puede ofrecer las mismas acciones aplicables como acceso adicional.

### Opciones TLS

- La persona puede activar TLS para una conexión MySQL.
- La validación del certificado del servidor está activa por defecto y puede usar las autoridades de confianza del sistema. Se admite indicar un certificado de autoridad propio cuando el servidor lo requiera.
- Se admite autenticación mutua mediante certificado de cliente y su clave privada. Si la clave requiere una frase de paso, se solicita y se guarda como secreto en el almacén de credenciales del sistema cuando la persona guarda la conexión.
- Las rutas a archivos de certificados y claves se pueden guardar en la configuración local; sus contenidos no se copian allí. Si un archivo necesario falta o no se puede leer, se informa el problema y la conexión no se abre.
- Si TLS está configurado como requerido y no se puede establecer o verificar, la conexión falla. La aplicación no continúa sin TLS de forma silenciosa.
- La interfaz muestra si la conexión abierta está usando TLS.

### Opciones del túnel SSH

- La persona puede indicar host SSH, puerto, usuario y método de autenticación.
- El MVP debe admitir autenticación SSH por contraseña, por archivo de clave privada y mediante un agente SSH disponible en el sistema. Si la clave privada está protegida, se puede proporcionar su frase de paso.
- Al elegir agente SSH, la aplicación no guarda ni solicita la clave privada. Si el agente no está disponible o no ofrece una identidad válida, se informa el error sin cambiar automáticamente a otro método de autenticación.
- La identidad del servidor SSH se verifica antes de abrir el túnel. Una clave de host desconocida o cambiada requiere una decisión explícita de la persona; nunca se acepta automáticamente.
- Los secretos SSH se guardan en el almacén de credenciales del sistema. La ruta del archivo de clave privada puede guardarse como configuración no secreta; el contenido de la clave no se copia a la configuración de DBSUAL.
- El túnel se cierra al desconectar o cuando falla la conexión. Un error del túnel no se presenta como un error de credenciales MySQL.
- TLS y SSH se pueden usar juntos. Activar SSH no desactiva TLS si este está configurado.

## Flujos

### Crear y probar

1. La persona elige **Nueva conexión** y el motor MySQL.
2. Elige el tipo (**Directa**, **DLL** o **HTTP Tunnel**) y completa los campos requeridos por ese transporte. La interfaz valida los campos antes de intentar conectar.
3. Puede elegir **Probar conexión**. La prueba informa éxito o un error concreto, sin guardar ni abrir permanentemente la conexión.
4. Elige **Guardar**. La conexión aparece en el explorador, incluso si todavía no se ha conectado.
5. Si decide guardar sin probar, la interfaz no debe indicar que la conexión ya funciona.

### Abrir y desconectar

1. Al abrir una conexión guardada, la aplicación recupera su secreto del almacén de credenciales, si existe, e intenta conectarse.
2. Durante el intento se muestra el estado **Conectando**.
3. El resultado cambia a **Conectada** o **Error**. El error deja disponible la acción de reintentar y editar la configuración.
4. Al desconectar, la aplicación cierra o libera los recursos de esa conexión y muestra **Desconectada**.

### Editar

1. La persona puede cambiar los datos de una conexión guardada.
2. La contraseña existente permanece protegida si el campo de contraseña se deja sin modificar.
3. Un cambio de parámetros que afectan la conexión exige volver a conectar para usar los nuevos valores.
4. Si el guardado falla, la configuración anterior sigue disponible y se muestra el error.

### Quitar una conexión

1. La persona elige **Quitar conexión** y confirma la acción.
2. La aplicación elimina la configuración local y la credencial vinculada a esa conexión.
3. Quitar una conexión no elimina bases de datos ni modifica el servidor remoto. La interfaz debe decirlo claramente.
4. Si se produce un fallo parcial al quitarla, la aplicación lo informa y permite reintentar la limpieza.

## Reglas y estados

- Estados visibles: **Desconectada**, **Conectando**, **Conectada** y **Error**.
- Una prueba de conexión no se confunde con el estado de una conexión abierta.
- El cierre de la aplicación libera las conexiones activas. Las conexiones guardadas permanecen para la siguiente apertura.
- Las contraseñas no se guardan en archivos de configuración, estado de Zustand, registros ni mensajes de error. Solo se mantienen en memoria el tiempo necesario para conectar.
- La interfaz identifica errores de datos incompletos, credenciales rechazadas, servidor no disponible y tiempo de espera agotado cuando el motor permita distinguirlos.
- Una conexión fallida no impide usar o administrar otras conexiones guardadas.
- Las llamadas entre React y Rust usan identificadores de conexión; no envían contraseñas de regreso a la interfaz después de guardarlas.

## Criterios de aceptación de MySQL

1. Dado un formulario con host, puerto o usuario inválido, al probar o guardar se indican los campos que hay que corregir y no se intenta la conexión.
2. Dado un servidor MySQL accesible y credenciales válidas, **Probar conexión** informa éxito sin dejar una conexión abierta ni guardar el formulario.
3. Dado un servidor inaccesible o credenciales inválidas, la prueba informa un error comprensible y no marca la conexión como conectada.
4. Dada una conexión guardada, al reiniciar la aplicación aparece con sus datos no secretos y permite abrirse usando la credencial guardada.
4a. Dadas varias conexiones guardadas, cada fila mantiene su propia acción **Conectar**/**Desconectar** y estado; operar sobre una no altera las demás.
4b. En cada fila, **Más opciones** agrupa **Editar** y **Quitar conexión**; quitar solicita confirmación y actúa sobre la conexión identificada.
5. Dada una conexión guardada, al editar solo su nombre la contraseña existente continúa funcionando.
6. Dada una conexión abierta, al desconectarla su estado cambia a **Desconectada** y dejan de ejecutarse operaciones sobre esa sesión.
7. Dada una conexión guardada, al quitarla desaparece de la lista y se elimina su credencial local sin ejecutar instrucciones de borrado en el servidor.
8. Los mensajes de error y registros de estos flujos no exponen contraseñas ni cadenas de conexión que las contengan.
9. Dada una conexión con TLS requerido, si el certificado del servidor no es válido o la negociación TLS falla, la aplicación muestra el motivo y no abre una conexión sin TLS.
10. Dada una conexión MySQL configurada con SSH y credenciales válidas, la aplicación abre el túnel, conecta al servidor MySQL a través de él y cierra el túnel al desconectar.
11. Dada una clave de host SSH desconocida o cambiada, la aplicación no abre el túnel sin una decisión explícita de la persona.
12. Dada una conexión que combina SSH y TLS, ambos mecanismos permanecen activos; el estado de conexión solo pasa a **Conectada** cuando se han establecido correctamente.
13. Dado un servidor que exige certificado de cliente TLS y archivos válidos configurados, la conexión se establece. Si faltan los archivos o son rechazados, se informa el error y no se continúa sin autenticación mutua.
14. Dada una conexión SSH configurada para usar agente, un agente disponible con una identidad autorizada permite abrir el túnel. Si no hay agente o identidad válida, la conexión falla con un mensaje específico.
15. Crear o editar una conexión permite elegir **Directa**, **DLL** o **HTTP Tunnel**, conserva la elección al guardar y muestra los campos pertinentes.
16. **Directa** usa host/puerto MySQL y respeta TLS; el túnel SSH opcional no cambia silenciosamente la verificación TLS.
17. Mientras DLL o HTTP Tunnel estén pendientes de implementación, seleccionarlos informa que no están disponibles y no permite guardar ni probar una conexión como si funcionara.

## Implementación inicial PostgreSQL — 2026-09-29

- El selector visual incluye PostgreSQL, sugiere el puerto 5432, conserva TLS con verificación de identidad activado por defecto y no muestra SSH como disponible. La prueba visual verifica el payload de motor/puerto/TLS usando IPC simulado.
- Rust incorpora un adaptador PgPool separado de MySqlPool, validación de conexión, confirmación del banner/versión PostgreSQL, detección de TLS y listado de bases a las que el rol tiene permiso CONNECT. Los comandos existentes de probar/abrir/desconectar y listar bases despachan por motor; editar o quitar una conexión también cierra su pool PostgreSQL.
- Esta porción es directa y de lectura. Túnel SSH y certificado mutuo TLS PostgreSQL se rechazan/no se ofrecen todavía. La prueba ignorada del adaptador pasó contra un contenedor desechable PostgreSQL 16: abrió una conexión directa sin TLS, identificó familia/versión y listó las bases accesibles. No prueba el comando Tauri ni la ventana nativa de Windows, TLS requerido, cuentas de permisos mínimos o la matriz completa; Playwright verifica el payload con IPC simulado.
- El catálogo PostgreSQL de bases, esquemas, tablas, vistas y columnas ya está disponible. El editor permite `SELECT` acotado en modo de solo lectura; aún faltan cancelación, cuadrícula, preparación/aplicación de cambios, historial y respaldo/restauración PostgreSQL.

## Extensión a MariaDB, PostgreSQL y SQLite

- MariaDB utiliza un formulario de red parecido a MySQL, pero se identifica como motor propio para aplicar sus reglas de autenticación, TLS, metadatos y respaldo. No se supone que un volcado de un motor pueda restaurarse en el otro sin validación. La UI permite crear conexiones MariaDB por separado; el core comprueba familia y versión y admite 10.6, 10.11 y 11.4. Una integración desechable verificó conexión, lectura, catálogo, estructura y metadatos del servidor en 10.6.28, 10.11.19 y 11.4.13. La edición de una conexión existente conserva el motor para no cambiar la interpretación de secretos guardados.
- PostgreSQL seguirá los mismos estados y operaciones de ciclo de vida, con sus propios parámetros de red y autenticación.
- SQLite utilizará una ruta a un archivo local en lugar de host, puerto y usuario. **Abrir conexión** validará que el archivo sea accesible.
- La creación de un archivo SQLite nuevo pertenece al spec de creación de bases de datos; este spec cubre abrir y guardar la referencia a un archivo existente.
- Cada motor tendrá criterios de aceptación propios antes de considerarse implementado.

## Fuera de este spec

- Listado de bases de datos y tablas.
- Crear o eliminar bases de datos.
- Importar o exportar datos.
- Editor y ejecución de consultas SQL.

## Decisiones pendientes

1. Formatos de certificados y claves que deberá admitir el MVP, y compatibilidad concreta con el agente SSH nativo de cada sistema operativo objetivo.
2. Si una conexión puede guardar una base de datos predeterminada opcional.
3. Qué comportamiento se espera cuando el sistema operativo no permite acceder al almacén seguro de credenciales.
4. Contrato DLL: ABI/biblioteca cliente, versiones, arquitectura, dependencias, procedencia y distribución.
5. Protocolo HTTP Tunnel compatible, URL, autenticación, validación del endpoint, TLS de extremo a extremo, timeouts, cancelación, límites y mapeo de errores.

## Avance de conexiones SQLite — 2026-09-30

Se pueden guardar, probar y abrir conexiones a archivos SQLite existentes mediante el selector nativo de archivos de Tauri. DBSUAL canonicaliza la ruta y abre el archivo en solo lectura; no crea bases nuevas ni almacena credenciales para SQLite. La selección requiere una prueba correcta antes de guardar como conexión válida. El recorrido del selector se debe verificar en Windows, macOS y Linux según el spec 019.

Corrección de implementación: guardar una conexión SQLite verifica el archivo en el core y conserva su ruta canonicalizada; el botón Probar conexión es opcional porque Guardar repite esa validación.
