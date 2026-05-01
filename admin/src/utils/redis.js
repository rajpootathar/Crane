const redis = require("redis");

let redisClient;

(async () => {
  redisClient = redis.createClient({
    socket: {
      host: process.env.REDIS_HOST || "127.0.0.1",
      port: process.env.REDIS_PORT || 6379,
      ...(process.env.REDIS_TLS_ENABLED === "true" && { tls: true }),
    },
    password: process.env.REDIS_PASSWORD,
    legacyMode: true
  });

  redisClient.on("error", (error) => console.error(`Error : ${error}`));

  await redisClient.connect();
})();

const fetchFromCache = async (key) => {
  return new Promise((resolve, reject) => {
    redisClient.get(key, (err, result) => {
      if (err) {
        err.status = 400;
        reject(err);
      }
      resolve(result);
    });
  });
};

const setOnCache = async (key, value, expires = null) => {
  const serializedValue =
    typeof value === "object" ? JSON.stringify(value) : value;

  if (expires) {
    await redisClient.set(key, serializedValue, "EX", expires);
  } else {
    await redisClient.set(key, serializedValue);
  }
};

const removeCache = async (key) => {
  await redisClient.del(key);
  return;
};

const removeAllCache = async (key) => {
  await redisClient.del(key);

  return;
};

const redisHSet = async (key, field, value) => {
  return new Promise((resolve, reject) => {
    redisClient.hset(key, field, JSON.stringify(value), (err, result) => {
      if (err) {
        err.status = 400;
        reject(err);
      }
      resolve(result);
    });
  });
};

const redisHGet = async (key, field) => {
  return new Promise((resolve, reject) => {
    redisClient.hget(key, field, (err, result) => {
      if (err) {
        err.status = 400;
        reject(err);
      }
      if (result) {
        resolve(JSON.parse(result));
      } else {
        resolve(false);
      }
    });
  });
};

module.exports = {
  fetchFromCache,
  setOnCache,
  removeCache,
  removeAllCache,
  redisHSet,
  redisHGet,
};
