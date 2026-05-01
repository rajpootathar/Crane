const bcrypt = require("bcryptjs");
const jwt = require("jsonwebtoken");
const { setOnCache, fetchFromCache } = require("../utils/redis");

const {
  AUTH_TOKEN_KEY,
  AUTH_TOKEN_EXPIRES,
  REFRESH_TOKEN_KEY,
  REFRESH_TOKEN_EXPIRES,
} = require("../config");

//Utility functions
(module.exports.GenerateSalt = async () => {
  return await bcrypt.genSalt();
}),
  (module.exports.GeneratePassword = async (password, salt) => {
    return await bcrypt.hash(password, salt);
  });

module.exports.ValidatePassword = async (
  enteredPassword,
  savedPassword,
  salt
) => {
  return (await this.GeneratePassword(enteredPassword, salt)) === savedPassword;
};

(module.exports.GenerateSignature = (payload) => {
  const token = jwt.sign(payload, AUTH_TOKEN_KEY, {
    expiresIn: AUTH_TOKEN_EXPIRES,
  });
  return token;
}),
  (module.exports.ValidateSignature = async (req) => {
    const signature = req.get("Authorization");

    if (signature && signature.split(" ")[0] === "Bearer") {
      const payload = await jwt.verify(signature.split(" ")[1], AUTH_TOKEN_KEY);
      req.user = payload;
      return true;
    }

    return false;
  });

module.exports.GenerateRefreshToken = (payload) => {
  const token = jwt.sign(payload, REFRESH_TOKEN_KEY, {
    expiresIn: REFRESH_TOKEN_EXPIRES,
  });
  return token;
};

module.exports.VerifyRefreshToken = (userId, refreshToken) => {
  return new Promise(async (resolve) => {
    const storedToken = await fetchFromCache(userId);
    if (refreshToken === storedToken) {
      resolve(true);
    } else {
      resolve(false);
    }
  });
};

module.exports.StoreRefreshTokenInRedis = async (userId, refreshToken) => {
  return await setOnCache(userId, refreshToken, 3600);
};
