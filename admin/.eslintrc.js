module.exports = {
  env: {
    node: true,
    es6: true,
  },
  extends: ["eslint:recommended", "plugin:node/recommended"],
  overrides: [
    {
      env: {
        node: true,
        es6: true,
      },
      files: [".eslintrc.{js}"],
      parserOptions: {
        sourceType: "script",
      },
    },
  ],
  parserOptions: {
    ecmaVersion: "latest",
    sourceType: "module",
  },
  plugins: ["node"],
  rules: {
    "no-unused-vars": "warn",
    "no-empty": "warn",
    "node/no-unsupported-features/es-syntax": ["error", { version: ">=8.3.0" }],
    "no-useless-catch": "off",
    "node/no-unpublished-require": "off",
    "no-case-declarations": "off",
    "no-process-exit": "off",
    "no-async-promise-executor": "off",
    "no-unexpected-multiline": "off",
  },
};
