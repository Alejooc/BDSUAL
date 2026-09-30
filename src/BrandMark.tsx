type BrandMarkProps = {
  className?: string
  title?: string
}

export default function BrandMark({ className, title }: BrandMarkProps) {
  return <svg className={className} viewBox="0 0 64 64" role={title ? 'img' : undefined} aria-hidden={title ? undefined : true}>
    {title && <title>{title}</title>}
    <path fill="#F4F6FA" fillRule="evenodd" d="M9 10h16c10.8 0 18 8.8 18 22s-7.2 22-18 22H9V10Zm11 11v22h4c5.2 0 8-4.1 8-11s-2.8-11-8-11h-4Z" />
    <path fill="#6847F5" fillRule="evenodd" d="M29 10h14c7.2 0 12 4.2 12 10.5 0 4.5-2.2 8-6.2 9.8C53.4 32 56 35.8 56 41c0 7.8-5.5 13-14 13H29V10Zm11 9v8h3c2.3 0 3.7-1.4 3.7-4s-1.4-4-3.7-4h-3Zm0 17v9h3.5c2.8 0 4.5-1.6 4.5-4.5S46.3 36 43.5 36H40Z" />
    <path fill="#F4F6FA" d="M21 26c0-3.3 4.9-5.8 11-5.8s11 2.5 11 5.8v13c0 3.3-4.9 5.8-11 5.8S21 42.3 21 39V26Z" />
    <path fill="#37D4E7" d="M21 26c0-3.3 4.9-5.8 11-5.8S43 22.7 43 26s-4.9 5.8-11 5.8S21 29.3 21 26Z" />
    <path fill="none" stroke="#141820" strokeWidth="1.5" d="M21 32.4c0 3.3 4.9 5.8 11 5.8s11-2.5 11-5.8M21 39c0 3.3 4.9 5.8 11 5.8S43 42.3 43 39" />
  </svg>
}
