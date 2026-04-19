import maxJsxProps from "./max-jsx-props.js";
import noInlineStyles from "./no-inline-styles.js";
import noDirectFetch from "./no-direct-fetch.js";
import noNonVitestTesting from "./no-non-vitest-testing.js";
import noJsFileExtension from "./no-js-file-extension.js";
import noRawUndefinedUnion from "./no-raw-undefined-union.js";

export default {
  rules: {
    "max-jsx-props": maxJsxProps,
    "no-inline-styles": noInlineStyles,
    "no-direct-fetch": noDirectFetch,
    "no-non-vitest-testing": noNonVitestTesting,
    "no-js-file-extension": noJsFileExtension,
    "no-raw-undefined-union": noRawUndefinedUnion,
  },
};
