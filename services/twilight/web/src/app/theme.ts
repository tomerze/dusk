import type { CSSVariablesResolver, MantineColorsTuple } from '@mantine/core'
import { createTheme, defaultVariantColorsResolver } from '@mantine/core'

const lightSchemeText: Record<string, string> = {
  red: '#b42318',
  orange: '#a8430a',
  yellow: '#7a5300',
  green: '#1f6f35',
  teal: '#0b6b60',
  cyan: '#0a6378',
  blue: '#1a56b0',
  grape: '#83399c',
  gray: '#4a5468',
  dusk: '#3e48b8',
}

const dusk: MantineColorsTuple = [
  '#eef0ff',
  '#dce0ff',
  '#b8c0ff',
  '#949ffc',
  '#7a86f5',
  '#6a76f0',
  '#5f6bee',
  '#4f5ad4',
  '#4550be',
  '#3843a8',
]

const slate: MantineColorsTuple = [
  '#e4e8f1',
  '#c3cada',
  '#98a2b8',
  '#7c86a0',
  '#3a4458',
  '#2c3547',
  '#1f2737',
  '#161d2b',
  '#111725',
  '#0c111c',
]

export const theme = createTheme({
  primaryColor: 'dusk',
  primaryShade: { light: 7, dark: 3 },
  autoContrast: true,
  luminanceThreshold: 0.18,
  colors: { dusk, dark: slate },
  fontFamily:
    "'Atkinson Hyperlegible Next Variable', system-ui, -apple-system, 'Segoe UI', sans-serif",
  fontFamilyMonospace:
    "'Atkinson Hyperlegible Mono Variable', ui-monospace, 'SFMono-Regular', Menlo, monospace",
  headings: {
    fontFamily:
      "'Atkinson Hyperlegible Next Variable', system-ui, -apple-system, 'Segoe UI', sans-serif",
    fontWeight: '650',
    sizes: {
      h1: { fontSize: '1.5rem', lineHeight: '1.25' },
      h2: { fontSize: '1.125rem', lineHeight: '1.3' },
      h3: { fontSize: '1rem', lineHeight: '1.35' },
      h4: { fontSize: '0.875rem', lineHeight: '1.4' },
    },
  },
  fontSizes: {
    xs: '0.75rem',
    sm: '0.8125rem',
    md: '0.875rem',
    lg: '1rem',
    xl: '1.25rem',
  },
  defaultRadius: 'sm',
  radius: { xs: '2px', sm: '4px', md: '6px', lg: '10px', xl: '16px' },
  focusRing: 'auto',
  variantColorResolver: (input) => {
    const resolved = defaultVariantColorsResolver(input)
    const primary = input.color === undefined || input.color === 'dusk'
    if (primary && (input.variant === 'filled' || input.variant === undefined)) {
      return { ...resolved, color: 'var(--twilight-on-accent)' }
    }
    return resolved
  },
  cursorType: 'pointer',
  components: {
    Tooltip: { defaultProps: { openDelay: 250, withArrow: true, multiline: true, maw: 360 } },
    Badge: {
      defaultProps: { radius: 'sm' },
      styles: { root: { textTransform: 'none', letterSpacing: 0 } },
    },
    Button: { defaultProps: { size: 'sm' } },
    TextInput: { defaultProps: { size: 'sm' } },
    Select: { defaultProps: { size: 'sm', allowDeselect: false } },
    NumberInput: { defaultProps: { size: 'sm' } },
    Modal: { defaultProps: { centered: true, radius: 'md' } },
    Paper: { defaultProps: { radius: 'md' } },
  },
})

export const cssVariablesResolver: CSSVariablesResolver = () => ({
  variables: {
    '--twilight-mono-feature': '"zero" 1',
  },
  light: {
    ...Object.fromEntries(
      Object.entries(lightSchemeText).flatMap(([color, value]) => [
        [`--mantine-color-${color}-light-color`, value],
        [`--mantine-color-${color}-text`, value],
      ]),
    ),
    '--twilight-on-accent': '#ffffff',
    '--mantine-color-body': '#f4f6fa',
    '--mantine-color-dimmed': '#59647d',
    '--mantine-color-default-border': '#d5dbe6',
    '--twilight-surface': '#ffffff',
    '--twilight-surface-raised': '#f8f9fc',
    '--twilight-surface-sunken': '#eceff5',
    '--twilight-border-subtle': '#e3e7ef',
    '--twilight-text-strong': '#151b29',
    '--twilight-row-hover': '#eef1f8',
    '--twilight-nav': '#eceff6',
    '--twilight-accent-soft': 'rgba(79, 90, 212, 0.1)',
    '--twilight-bar': 'rgba(79, 90, 212, 0.2)',
  },
  dark: {
    '--twilight-on-accent': '#11152b',
    '--mantine-color-body': '#121826',
    '--mantine-color-dimmed': '#98a2b8',
    '--mantine-color-default-border': '#2f394c',
    '--twilight-surface': '#171e2d',
    '--twilight-surface-raised': '#1c2435',
    '--twilight-surface-sunken': '#0f1421',
    '--twilight-border-subtle': '#252e40',
    '--twilight-text-strong': '#f1f4fa',
    '--twilight-row-hover': '#1e2739',
    '--twilight-nav': '#0f1421',
    '--twilight-accent-soft': 'rgba(148, 159, 252, 0.12)',
    '--twilight-bar': 'rgba(148, 159, 252, 0.26)',
  },
})
