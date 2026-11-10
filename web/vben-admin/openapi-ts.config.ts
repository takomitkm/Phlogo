// OpenAPI -> TypeScript generation (p6-01). Input is the Rust utoipa export;
// regenerate with `pnpm openapi:gen` whenever the API contract changes.
export default {
  input: '../../api/openapi/spec.json',
  output: 'src/api/generated',
};
