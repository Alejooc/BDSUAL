# Spec 020 — Apariencia, temas y colores

Estado: selección de temas y personalización de paleta en implementación inicial; tema claro pendiente de revisión visual en ventana nativa
Versión: 0.1
Última actualización: 2026-09-30
Depende de: [005 — Interfaz de trabajo](005-interfaz-de-trabajo.md), [009 — Arquitectura](009-arquitectura.md), [017 — Base Windows](017-base-aplicacion-windows.md), [019 — Plataformas](019-plataformas.md)

## Objetivo

Dar a cada persona control práctico sobre la apariencia de DBSUAL mediante temas completos, controles de legibilidad y una paleta personalizada. Los temas deben cubrir todos los componentes de la interfaz, incluido el editor SQL y los elementos nativos, y conservarse después de cerrar y abrir la aplicación.

## Ubicación y persistencia

- Las opciones viven en **Configuración > Apariencia**, dentro de la página completa de configuración.
- La selección de tema, escala de texto y colores personalizados se aplica inmediatamente y se guarda en las preferencias locales no secretas.
- Las preferencias antiguas que no contengan colores personalizados se cargan con una paleta predeterminada válida sin perder los demás ajustes.
- Al seleccionar un tema predeterminado no se borran los colores personalizados guardados. Al volver a **Personalizado**, se recupera esa paleta.
- Restablecer preferencias devuelve el tema DBSUAL oscuro, el tamaño de texto estándar y los colores personalizados predeterminados.

## Temas predeterminados

La primera entrega ofrece estas paletas completas:

1. **Oscuro DBSUAL**: tema oscuro original, refinado con tokens consistentes.
2. **Claro**: superficies claras con texto, bordes, controles, tablas, menús y editor legibles; debe ser utilizable en toda la aplicación.
3. **Alto contraste**: contraste reforzado para texto, divisores, foco y controles.
4. **Medianoche**: azul oscuro con énfasis azul claro.
5. **Nórdico**: grises azulados y acento celeste.
6. **Bosque**: verdes oscuros con acento verde claro.
7. **Personalizado**: edición de los tokens de color descritos abajo.

Cada opción muestra una miniatura de su paleta y su nombre. La selección actual se identifica mediante borde/estado accesible además del color.

## Personalización de colores

La paleta personalizada expone selectores con valor hexadecimal para:

- Fondo principal.
- Panel lateral.
- Superficies y barras.
- Controles y elementos elevados.
- Bordes y separadores.
- Texto principal.
- Texto secundario.
- Color de énfasis y foco.

El selector cambia automáticamente al modo **Personalizado** al editar un color. La vista previa se aplica de inmediato. Los valores se validan como colores hexadecimales `#RRGGBB` tanto en la interfaz como en Rust; datos inválidos se rechazan con un error claro y sin reemplazar la preferencia válida anterior.

## Cobertura visual

Los tokens se aplican coherentemente a la barra de título y actividad, explorador, pestañas, panel central, cuadrículas, filtros, resultados SQL, Monaco, menús contextuales, diálogos, controles, scrollbars y estados vacíos/de error. No deben quedar superficies oscuras con texto oscuro o texto claro sobre superficies claras por estilos específicos de un componente.

El tema claro es un criterio bloqueante: debe revisarse en ventana Tauri y corregirse en cada componente visible hasta que todos los recorridos comunes se puedan leer y usar. Cambiar el tema no modifica datos de conexión, consultas, resultados, historial ni contenido de una base.

## Accesibilidad y comportamiento

- Texto, controles y foco mantienen contraste legible; el foco visible usa el color de énfasis y no depende solo del color para indicar selección.
- Los selectores tienen nombres accesibles y muestran el valor actual en hexadecimal.
- La escala tipográfica existente se mantiene independiente de la selección de colores.
- Monaco utiliza el tema claro cuando la aplicación está en Claro y el tema oscuro en las paletas oscuras; el contenido y la posición del cursor permanecen intactos al cambiar.
- Si un componente no admite de forma segura una paleta personalizada, usa un token del sistema o un valor accesible equivalente; no queda ilegible.

## Criterios de aceptación

1. Configuración muestra todas las paletas predeterminadas con nombre y muestra de color; cambiar entre ellas actualiza la aplicación sin reiniciarla.
2. El tema Claro es legible y utilizable en explorador, conexiones, cuadrículas, filtros, editor SQL, resultados, menús y diálogos en la ventana nativa.
3. Alto contraste, Medianoche, Nórdico y Bosque aplican sus tokens de forma coherente a las mismas áreas.
4. Personalizado permite ajustar individualmente los ocho tokens; el cambio se refleja al instante y persiste después de reiniciar.
5. Cambiar a un tema predeterminado y regresar a Personalizado conserva la paleta editada.
6. Los valores de color inválidos se rechazan en Rust y no corrompen ni reemplazan la última preferencia válida.
7. Preferencias guardadas antes de la existencia de `customColors` se abren con defaults seguros y sin perder tema, escala de fuente ni demás preferencias.
8. La selección del tema, los colores, estados de foco y controles son accesibles por teclado y no comunican su estado solo por color.
9. Las opciones de apariencia se aplican únicamente a la interfaz y nunca ejecutan operaciones sobre una base de datos.
10. Se comprueban legibilidad y controles en el WebView nativo de cada sistema que se declare compatible, según la matriz del spec 019.

## Estado de implementación

Existe una implementación inicial en Configuración con seis paletas predeterminadas más Personalizado, ocho selectores de color y persistencia en preferencias. Rust valida el formato hexadecimal; las hojas de estilo aplican tokens a las áreas principales y Monaco cambia a su paleta clara u oscura según la luminancia relativa del fondo. `src/theme.ts` comparte la clasificación de luminancia entre Monaco y los controles nativos del WebView. `tests/e2e/appearance.spec.ts` comprueba selección accesible, aplicación del tema, edición de un fondo gris medio que requiere modo claro, preservación al alternar paletas y guardado de la preferencia mediante IPC simulado. Pruebas Vitest cubren fondos claros, oscuros, saturados y malformados; pruebas Rust comprueban rechazo de formatos hexadecimales inválidos y compatibilidad con preferencias antiguas sin `customColors`, preservando tema, sección y escalas. Las pruebas focalizadas, `npm run build`, `npm run test:unit` y el E2E de apariencia pasaron en Windows el 2026-09-30. La revisión visual en ventana nativa, la validación integral de contraste por componente y la matriz macOS/Linux siguen pendientes; no se consideran satisfechas solo por compilar.

## Decisiones pendientes

1. Para las paletas predeterminadas se fijan los umbrales del criterio 8: texto normal y secundario ≥ 4.5:1; acento en sus usos visibles ≥ 3:1. La comprobación de tokens no sustituye la revisión por componente ni en ventana nativa.
2. Determinar si se soportan importación/exportación de paletas de usuario en una entrega posterior.
3. Definir una política para compartir colores entre personalizaciones sin incluir datos privados de la aplicación.

### Umbrales y evidencia de paletas (2026-09-30)

Se usan los umbrales WCAG de contraste: 4.5:1 para texto normal (incluido texto secundario) y 3:1 para el token de acento en usos de texto grande o elementos gráficos/controles. `src/theme.test.ts` lee las reglas de paleta reales en `src/styles.css` y verifica texto, texto secundario y acento sobre fondo principal, lateral, superficie y superficie elevada en las seis paletas predeterminadas. Vitest pasó (4 pruebas en ese archivo). Esta comprobación cubre los tokens base; no inspecciona todas las excepciones CSS de cada componente ni acredita legibilidad del WebView nativo. La inspección de esos recorridos y la matriz macOS/Linux siguen pendientes.
