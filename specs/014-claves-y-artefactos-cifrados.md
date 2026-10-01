# Spec 014 — Claves y artefactos cifrados

Estado: implementación parcial
Versión: 0.3
Última actualización: 2026-09-29
Depende de: [009 — Arquitectura](009-arquitectura.md), [010 — Respaldos y recuperación](010-respaldos-y-recuperacion.md), [013 — Respaldos SQLite](013-respaldos-sqlite.md)

## Objetivo

Permitir que DBSUAL proteja respaldos y datos anteriores de forma consistente en Windows, macOS y Linux, y que la persona pueda recuperarlos tras reinstalar o cambiar de sistema operativo mediante una frase o un archivo de clave protegido. El formato cifrado es independiente del almacén local y un respaldo debe poder importarse aunque no exista el SQLite local de historial.

## Alcance y límite de protección

- Se cifran los respaldos completos, los archivos de entrada que contienen datos y las imágenes anteriores de filas. Los metadatos no secretos del historial local permanecen en SQLite según el spec 009. Rutas y permisos siguen las convenciones de datos privados de cada sistema.
- El cifrado protege principalmente los artefactos copiados o extraídos del disco. Una sesión de usuario comprometida mientras DBSUAL está abierta puede acceder a las bases o a claves en uso; el cifrado de archivos no resuelve ese caso.
- Una exportación SQL o CSV legible que la persona solicita es distinta del respaldo protegido. Se advierte su formato antes de guardarla en la ruta elegida.

## Modelo de claves

1. DBSUAL genera una **clave maestra del depósito** aleatoria de 256 bits. El core la guarda para uso habitual en el almacén seguro nativo del usuario: Windows Credential Manager, macOS Keychain o Secret Service en Linux. El historial solo almacena su identificador, nunca su valor. La abstracción y la matriz real se concretan en el spec 019.
2. Cada artefacto recibe una **clave de datos** aleatoria e independiente de 256 bits. El contenido se cifra con esa clave; la clave de datos se envuelve con la clave maestra y se almacena cifrada en el paquete del artefacto.
3. La frase de recuperación la genera DBSUAL con al menos 128 bits de entropía y un mecanismo para detectar errores de transcripción. No se acepta una frase libre elegida por la persona como única vía. La frase deriva una clave de envoltura con Argon2id y una sal única; el paquete incluye una copia cifrada de la clave maestra recuperable con ella.
4. El **archivo de clave exportable** contiene el identificador del depósito y otra copia cifrada de la clave maestra, protegida por una contraseña elegida al exportarlo. Se usa Argon2id con una sal distinta. El archivo no contiene la contraseña ni una clave maestra legible.
5. La frase y el archivo son vías independientes: cualquiera de los dos permite recuperar la clave maestra necesaria para abrir los artefactos del depósito. La contraseña del archivo no es la frase de recuperación.

La configuración inicial muestra y verifica la frase antes del primer respaldo; también ofrece crear y probar el archivo de clave. Ambas vías permanecen disponibles para la persona, y al menos una debe estar configurada antes de guardar un respaldo necesario para una operación destructiva. DBSUAL recomienda conservar ambas fuera de la carpeta de datos de la aplicación.

## Configuración en la interfaz

- La administración del depósito se encuentra en **Configuración de la aplicación > Recuperación y cifrado**, accesible desde la barra de menús superior conforme al spec 005.
- Desde esa sección la persona puede crear y confirmar la frase de recuperación, importar un archivo de clave existente, exportar/renovar un archivo de clave y consultar el estado de disponibilidad del depósito. La frase existente nunca se revela de nuevo.
- La página **Inicio/Bienvenida** no contiene las herramientas de claves; puede cerrarse sin afectar la configuración ya guardada.
- Cuando un respaldo u operación protegida requiere una clave que falta, DBSUAL explica el requisito y ofrece abrir directamente **Recuperación y cifrado**. Al completar o cancelar la configuración, conserva el destino y permite regresar al flujo anterior.
- Si no hay depósito configurado, se mantiene disponible la inspección y cualquier operación que no lo requiera; no se permite crear un respaldo protegido ni aplicar cambios destructivos fingiendo que la protección está activa.

## Formato portable del artefacto

- Cada respaldo se compone de **datos cifrados** y un **manifiesto versionado**. El manifiesto contiene formato, identificador del depósito y del artefacto, motor y versión, algoritmo, parámetros de derivación, sales, valores aleatorios, claves envueltas, tamaños y hash. Nunca contiene una clave o contraseña legible.
- Para MySQL, MariaDB y PostgreSQL, el flujo del proveedor se cifra durante su escritura. El formato de flujo autenticado incluye orden y fin de datos para detectar modificación, truncamiento o intercambio de bloques.
- Para SQLite, el archivo de datos es la base SQLCipher creada directamente según el spec 013. Su manifiesto acompaña obligatoriamente al archivo: sin él la frase no puede reconstruir la clave maestra y DBSUAL no debe presentar el respaldo como portable.
- **Exportar respaldo portable** entrega ambos componentes como una unidad verificable. La importación comprueba integridad, versión y vínculo entre manifiesto y datos antes de ofrecer restauración. Una copia aislada del SQLite local de historial no sustituye este paquete.
- El nombre original de la base y otros metadatos que puedan ser sensibles se minimizan en el manifiesto público; el inventario detallado se guarda dentro del contenido cifrado cuando sea viable.

## Primitivas y manejo de errores

- Los artefactos de flujo y las envolturas de claves usan cifrado autenticado AES-256-GCM mediante una biblioteca mantenida. Cada clave de datos se usa con valores aleatorios o contadores únicos según un formato de flujo versionado; nunca se reutiliza una pareja clave y nonce. Los metadatos críticos y el orden de bloques se autentican. El archivo SQLite usa el formato y la comprobación de integridad propios de SQLCipher, como define el spec 013.
- Para la contraseña del archivo y la frase se fija como perfil inicial Argon2id con sal de 128 bits, salida de 256 bits, 64 MiB de memoria, tres pasadas y cuatro vías. Los parámetros exactos se guardan en el manifiesto o en el archivo de clave, según la vía, para permitir futuras versiones; rendimiento y memoria se comprueban en cada plataforma objetivo antes de cerrar el perfil del MVP.
- Las claves se generan con un generador criptográfico del sistema. El core limita su tiempo en memoria y no las envía al frontend después de configurarlas ni a argumentos de proceso o registros.
- Si faltan el manifiesto, el archivo, la clave o una versión compatible, o falla la autenticación, se informa del motivo sin modificar la base de destino. Un hash correcto por sí solo no reemplaza la autenticación ni una prueba de restauración.
- Cambiar la contraseña del archivo produce un nuevo archivo de clave; no reescribe los respaldos. Cambiar la frase o la clave maestra requiere migrar las envolturas de los artefactos y verificar todos los paquetes antes de retirar la vía anterior.

## Decisiones de implementación

- El núcleo usa `aes-gcm 0.11.1` para envolturas AES-256-GCM, `argon2 0.6.0` con el perfil fijado arriba, `getrandom 0.4.3` para aleatoriedad del sistema y `zeroize 1.9.0` para reducir el tiempo de claves en memoria.
- La frase de recuperación usa BIP-39 en español: 128 bits de entropía crean doce palabras con checksum. Al leerla, el núcleo normaliza y verifica idioma, cantidad de palabras y checksum; deriva la clave con Argon2id, no con la derivación de semilla propia de BIP-39.
- Implementado parcialmente en `src-tauri/src/vault.rs`: generación de clave maestra y frase, envoltura autenticada de la clave maestra, restauración y validación. Windows Credential Manager está configurado; Cargo selecciona macOS Keychain y Linux Secret Service para sus respectivos targets, pero esos backends todavía no se compilaron ni probaron en sus sistemas nativos. El formato y las interfaces criptográficas son neutrales al sistema. Las operaciones de keyring son síncronas y deben llamarse desde un worker bloqueante; los accesos a la credencial del depósito se serializan.
- Implementado parcialmente un contenedor binario versionado para flujos: cabecera fija con depósito/artefacto, suite y perfil Argon2id, y claves envueltas; bloques AES-GCM de 64 KiB; clave de datos independiente por artefacto; prefijo aleatorio de nonce más contador de 64 bits para que no se repita un nonce bajo esa clave; AAD vincula hash de cabecera, artefacto, índice y longitud; pie autenticado verifica bytes y cantidad de bloques. El lector limita tamaño y secuencia, rechaza perfiles no admitidos, nonces inesperados, reordenamiento, truncado y bytes sobrantes.
- El lector entrega cada bloque solo después de validar su tag, pero el llamador debe mantener su destino provisional hasta validar el pie final. Los escritores de artefacto y descifrado usan un temporal hermano; el descifrado lo publica solo tras validar pie y EOF. En Windows la publicación usa `MoveFileExW` sin permiso para reemplazar y con escritura duradera, así que es atómica, rechaza destinos existentes y también funciona en unidades sin hard links. Los temporales se limpian ante error.
- Implementados los núcleos `export_key_file` e `import_key_file`: sobre JSON acotado a 4 KiB, versión y perfil Argon2id incluidos, AES-GCM vinculado al ID del depósito y contraseña de al menos 12 caracteres. El archivo contiene texto cifrado, sal y nonce; no incluye contraseña ni clave legible.
- `decrypt_artifact_with_key_file` conecta la importación de la clave con la apertura de un artefacto y verifica que ambos pertenezcan al mismo depósito. `decrypt_artifact_file_with_key_file` usa staging provisional y publica solo tras validar el pie autenticado.
- La inicialización y confirmación de la frase están conectadas mediante IPC y disponibles desde **Configuración > Recuperación y cifrado**, separadas de Inicio. Playwright verifica la ubicación de la sección en preview; el navegador no ejecuta las acciones IPC. El core conserva la frase pendiente solo en memoria, exige recuperarla antes de guardar la clave maestra en el almacén seguro del sistema y persiste únicamente la envoltura cifrada en el SQLite local. Cancelar o cerrar antes de confirmar descarta la configuración pendiente.
- La exportación e importación del archivo de clave están conectadas a selectores nativos. La importación pide la contraseña solo en memoria, rechaza un archivo de otro depósito y prepara una frase nueva para la misma clave y los mismos artefactos; la clave no se guarda en Windows hasta que la persona confirma esa frase. El ID del depósito se conserva como metadato. Los planes SQL individuales pueden prepararse y leerse desde el depósito local como artefactos cifrados; aún no hay proveedor de respaldo/restauración por motor ni UI para abrir un artefacto portable con una frase existente. Ninguna operación de escritura de bases se habilita con esta porción.
- `encrypt_artifact_reader_to_directory` conecta un flujo `Read` directamente con el contenedor autenticado, publica solo después de descifrar/verificar el temporal y devuelve bytes cifrados y hash SHA-256. Una prueba verifica flujo de varios bloques, hash del archivo, round-trip y que el ciphertext no contiene el patrón del contenido original. El ensamblador interno MySQL 8.4 también se comprobó con round-trip cifrado y rechazo de captura parcial. No hay formato portable de manifiesto ni restauración por motor; el stream MySQL actual no es un respaldo utilizable.
- Pruebas locales: `cargo test --locked` (29 pruebas), `npm run build` y `npm run test:e2e` (5 pruebas) pasaron el 2026-09-28. Incluyen round-trip criptográfico del archivo, rechazo de contraseña errónea, apertura de la clave importada con una frase nueva, persistencia del ID y borrado de plaintext tras probar escritura cifrada. La selección real del archivo y el IPC en una ventana Tauri nueva aún requieren recorrido manual en Windows.

## Recuperación tras reinstalar o cambiar de plataforma

1. La persona importa un paquete portable y aporta **la frase** o **el archivo de clave más su contraseña**.
2. DBSUAL recupera la clave maestra en memoria, abre y verifica el artefacto y muestra el contenido recuperable. No escribe la clave en el historial local.
3. Solo tras confirmar la recuperación, guarda la clave maestra en el almacén seguro nativo del nuevo perfil y vincula el paquete a un historial local nuevo o recuperado.

El archivo de clave y la frase son portables entre Windows, macOS y Linux. La credencial del almacén nativo queda asociada al perfil y sistema donde se guardó, por lo que no se copia ni migra entre sistemas.
4. Restaurar la base de datos sigue el spec del motor y, por defecto, crea un destino nuevo.

## Criterios de aceptación

1. Un respaldo portable se abre en otra instalación compatible de Windows, macOS o Linux con el paquete y la frase, sin credenciales ni SQLite de historial de la instalación original.
2. El mismo respaldo se abre con el paquete y el archivo de clave más su contraseña, sin usar la frase.
3. Una frase o contraseña errónea, un manifiesto alterado, un bloque truncado o un archivo de datos sustituido fallan sin tocar el destino.
4. El archivo de clave no permite recuperar el respaldo sin su contraseña; el paquete por sí solo tampoco.
5. Una copia SQLCipher sin su manifiesto se identifica como incompleta para recuperación portable.
6. Las claves y contraseñas no aparecen en IPC de resultados, argumentos de procesos auxiliares, registros ni SQLite de metadatos.
7. Tras cambiar la contraseña del archivo, tanto el archivo nuevo como el anterior siguen comportándose según las claves que contienen; el cambio no altera artefactos existentes.

## Criterios probados en la implementación parcial

- La frase generada contiene doce palabras españolas y recupera la misma clave maestra.
- Una frase inválida, una envoltura alterada, una envoltura asociada a otro identificador de depósito o campos con tamaños incorrectos se rechazan.
- Espacios adicionales en la entrada se normalizan antes de verificar y derivar.
- Un flujo de más de dos bloques se cifra/descifra con tamaño de bloque acotado y bytes idénticos.
- Alterar ciphertext, nonce o tag final, reordenar bloques, declarar longitudes excesivas, truncar el cierre o añadir bytes al final provoca error.
- El archivo de clave recupera y descifra un artefacto con su contraseña, de forma independiente de la frase; una contraseña equivocada, una clave alterada, un depósito distinto o una contraseña de exportación demasiado corta se rechazan.
- El escritor de archivo publica solo tras releer y validar el paquete completo; si el destino ya existe, lo conserva y falla.
- En Windows, `verify_artifact_from_keyring` recorre y autentica todos los bloques y el cierre final sin conservar el texto descifrado. La prueba acepta el artefacto válido y rechaza el tag final alterado. Esta verificación previa aún no está conectada a la restauración MySQL ni prueba recuperación después de reinstalar Windows.

En Windows, el 2026-09-29 `cargo test --manifest-path src-tauri/Cargo.toml --locked` pasó 42 pruebas con una integración MySQL ignorada por defecto; incluye round-trip temporal en el Administrador de credenciales y la autenticación previa de artefacto. `npm run build` y seis recorridos Playwright también pasaron. La tarjeta del depósito está deshabilitada en preview web. El selector nativo y el recorrido después de reinstalar Windows no se probaron manualmente; tampoco hay restauración de una base real.

## Portabilidad y disponibilidad del almacén de claves

- Las copias portables se recuperan con su frase o archivo de clave aunque se cambie entre Windows, macOS y Linux. Recuperar la copia no requiere exportar ni migrar el elemento del almacén seguro del sistema anterior.
- Si el almacén nativo no está disponible, bloqueado o sin sesión de usuario (caso posible en Linux headless), DBSUAL muestra un error claro. Nunca persiste secretos en SQLite, archivos de preferencias, logs ni variables de entorno como alternativa silenciosa. Solo puede ofrecer una vía explícita de recuperación/entrada temporal si el flujo correspondiente la implementa.
- Un elemento guardado por DBSUAL se aísla mediante servicio e identificador de cuenta estables por instalación/perfil. Las pruebas comprueban guardar, leer, sobrescribir y eliminar sin tocar credenciales ajenas.

## Decisiones técnicas pendientes

1. Añadir importación UI del archivo de clave y recuperación de artefactos con selector de ruta; probar inicialización/exportación nativas de extremo a extremo.
2. Integrar el contenedor en los proveedores de respaldo y restauración MySQL; no habilitar operaciones destructivas antes de demostrar recuperación real.
3. Empaquetado portable del manifiesto y el archivo SQLCipher, incluida recuperación tras caída.
4. Pruebas de rendimiento Argon2id/cifrado y recuperación tras reinstalación o cambio de sistema entre Windows, macOS y Linux.

## Referencias técnicas

- [Microsoft: credenciales genéricas de Windows](https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-credwritew).
- [Microsoft: límites de DPAPI entre usuarios y equipos](https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptprotectdata).
- [Microsoft: recomendaciones de cifrado autenticado y nonces](https://learn.microsoft.com/en-us/security/engineering/cryptographic-recommendations).
- [RFC 9106: Argon2id](https://datatracker.ietf.org/doc/html/rfc9106).
- [RustCrypto AES-GCM](https://docs.rs/aes-gcm/0.11.1/aes_gcm/), [Argon2](https://docs.rs/argon2/0.6.0/argon2/) y [BIP-39 con español](https://docs.rs/bip39/2.2.2/bip39/enum.Language.html).
- [OWASP: almacenamiento criptográfico y gestión de claves](https://cheatsheetseries.owasp.org/cheatsheets/Cryptographic_Storage_Cheat_Sheet.html).
