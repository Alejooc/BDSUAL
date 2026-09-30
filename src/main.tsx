import ReactDOM from 'react-dom/client'
import { StrictMode } from 'react'
import '@fontsource/dm-mono/latin-400.css'
import '@fontsource/dm-mono/latin-500.css'
import '@fontsource/dm-sans/latin-400.css'
import '@fontsource/dm-sans/latin-500.css'
import '@fontsource/dm-sans/latin-600.css'
import '@fontsource/manrope/latin-600.css'
import '@fontsource/manrope/latin-700.css'
import App from './App'
import './styles.css'

ReactDOM.createRoot(document.getElementById('root')!).render(<StrictMode><App /></StrictMode>)
