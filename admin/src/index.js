const express = require("express");
const bigintSerialization = require("../../shared/src/utils/bigint-serialization");

// Handle BigInt serialization
bigintSerialization();
const { PORT } = require("./config");
const expressApp = require("./express-app");

const StartServer = async () => {
  const app = express();

  await expressApp(app);

  app
    .listen(PORT, () => {
      console.log(`listening to port ${PORT}`);
    })
    .on("error", (err) => {
      console.log(err);
      process.exit();
    });
};

StartServer();
