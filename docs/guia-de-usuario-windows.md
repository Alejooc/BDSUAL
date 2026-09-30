# Guía de usuario de DBSUAL para Windows

DBSUAL es un espacio de trabajo de escritorio para conectarse a bases de datos, explorar sus objetos y ejecutar consultas de lectura. Esta guía describe las funciones disponibles en la versión actual del MVP.

## Abrir DBSUAL

El proyecto genera paquetes de Windows MSI y NSIS dirigidos a x64. CI comprueba que cada paquete exista, tenga tamaño no vacío y metadatos coherentes con el producto y su versión; también comprueba la plataforma declarada por MSI y que el lanzador NSIS tenga formato PE válido. Esto comprueba la salida del empaquetado, pero no instala ni ejecuta el paquete. Los instaladores no están firmados para publicación. La instalación en Windows limpio, la actualización desde una versión anterior, la conservación de datos durante la actualización y la desinstalación siguen pendientes de validación manual antes de distribuirlos.

## Crear una conexión

1. En **Explorador**, selecciona **Nueva conexión**.
2. Elige MySQL, MariaDB, PostgreSQL o SQLite.
3. Para MySQL, MariaDB y PostgreSQL, indica el nombre, host, puerto, usuario y contraseña. Activa TLS si el servidor lo exige; DBSUAL verifica la identidad del servidor antes de continuar. MySQL y MariaDB también permiten configurar un túnel SSH.
4. Para SQLite, elige un archivo `.db`, `.sqlite` o `.sqlite3` que ya exista. DBSUAL canonicaliza la ruta y lo abre en modo de solo lectura.
5. Usa **Probar conexión** para revisar el acceso y selecciona **Guardar**. La contraseña de los motores de servidor se guarda en el Administrador de credenciales de Windows; no se guarda en la configuración local.
6. Selecciona la conexión para abrirla. Una conexión SQLite muestra la base `main`; MySQL, MariaDB y PostgreSQL muestran las bases a las que la cuenta tiene acceso.

Si la conexión falla, revisa el host, puerto, usuario, contraseña y estado del servidor. Para TLS, confirma que el certificado y el nombre del servidor sean válidos. DBSUAL no omite errores de certificado para forzar una conexión.

## Explorar tablas y consultar datos

- Expande una base de datos para ver sus tablas y vistas; expande un objeto para cargar sus columnas.
- Selecciona una tabla para abrir su cuadrícula. Puedes ordenar, filtrar y recorrer páginas. `NULL` se muestra separado de una cadena vacía. En SQLite, los valores binarios se muestran en hexadecimal.
- Las conexiones MySQL, MariaDB y PostgreSQL pueden usarse en el editor SQL para ejecutar consultas `SELECT`. SQLite admite también `SELECT` en el editor sobre `main`.
- Las consultas se ejecutan con límites de tiempo y tamaño de resultado. Si se supera un límite, ajusta la consulta o consulta menos filas.
- Las cuadrículas y el editor SQL son de solo lectura en esta versión. No uses el editor para intentar modificar datos.

## Historial y recuperación

DBSUAL mantiene un historial local independiente de Git. El historial y los artefactos protegidos requieren configurar el depósito de recuperación desde **Configuración → Seguridad y recuperación** y guardar su frase fuera del equipo.

La captura y restauración disponibles para MySQL y MariaDB cubren un subconjunto de objetos: tablas, vistas locales dependientes y triggers simples admitidos. Una restauración se prepara en una base nueva, se revisa, se confirma en el historial y luego se aplica. Verifica el nombre de destino antes de aplicar.

La cobertura parcial no es un respaldo completo. No habilita eliminación de bases, importaciones destructivas ni aplicación general de SQL o edición de filas. PostgreSQL aún no ofrece respaldo/restauración protegidos. SQLite no ofrece historial de recuperación, importación, exportación SQL, eliminación de archivos ni cambios: esas funciones esperan la validación del respaldo cifrado SQLCipher y WAL.

Si DBSUAL muestra **Estado incierto** o un error durante una aplicación, no repitas la operación a ciegas. Actualiza el historial y comprueba el destino antes de preparar un nuevo cambio.

## Datos locales y seguridad

El archivo interno de DBSUAL conserva preferencias, sesión y metadatos no secretos de las conexiones. Las contraseñas de servidores se almacenan en el Administrador de credenciales de Windows. La frase de recuperación no se puede volver a mostrar después de completar su configuración; conserva la copia en un lugar seguro separado del equipo.

El historial no se sincroniza con GitHub o GitLab. Git puede usarse por separado para scripts revisables, pero no contiene las credenciales ni equivale a un respaldo restaurable de la base.

## Alcance pendiente

La versión actual sigue en desarrollo. CI verifica la construcción y metadatos básicos de los paquetes MSI/NSIS, pero no demuestra una instalación funcional. La instalación en Windows limpio, la actualización desde una versión anterior, la conservación de los datos locales, la desinstalación y la limpieza o conservación de datos asociada aún requieren validación manual. También requieren validación los recorridos de la ventana nativa con servidores reales, los permisos mínimos, las matrices completas de versiones y la recuperación íntegra por motor.
