/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        studio: {
          950: "#09090b",
          900: "#121215",
          850: "#18181b",
          800: "#202024",
          700: "#2e2e34",
          600: "#3f3f46",
          500: "#71717a",
          400: "#a1a1aa",
          100: "#f4f4f5",
        },
        brand: {
          primary: "#6366f1",
          hover: "#4f46e5",
          emerald: "#10b981",
          rose: "#f43f5e",
          amber: "#f59e0b",
        }
      },
      fontFamily: {
        sans: [
          "-apple-system",
          "BlinkMacSystemFont",
          '"Segoe UI"',
          "Roboto",
          '"Helvetica Neue"',
          "Arial",
          "sans-serif"
        ],
        mono: [
          "ui-monospace",
          "SFMono-Regular",
          "Menlo",
          "Monaco",
          "Consolas",
          "monospace"
        ]
      }
    },
  },
  plugins: [],
}
