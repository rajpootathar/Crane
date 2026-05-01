const Base = {};

Base.hasLookup = (lookups, code) => {
  if (!code) {
    return false;
  }

  let codeToFind;

  if (typeof code === "string") {
    codeToFind = code;
  } else {
    codeToFind = code.code;
  }

  const lookup = lookups.find(
    (x) => x.code.toLowerCase() === codeToFind.toLowerCase()
  );

  return !!lookup;
};

Base.lookupFinder = (lookups, code, defaultLookup = null) => {
  if (!code) {
    return defaultLookup;
  }

  let codeToFind;

  if (typeof code === "string") {
    codeToFind = code;
  } else {
    codeToFind = code.code;
  }

  return (
    lookups.find((x) => x.code.toLowerCase() === codeToFind.toLowerCase()) ||
    defaultLookup
  );
};

module.exports = Base;
