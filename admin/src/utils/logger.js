const moment = require("moment");
const fs = require("fs");
const path = require("path");
const { createLogger, format, transports } = require("winston");
const { combine, timestamp, printf, json, errors } = format;

const apiLoggerHandler = (module, type) => {
  var folderDate = moment(new Date()).format("DD-MM-YYYY");
  let logDir = path.normalize(
    __dirname + `/../../apiLogs/${folderDate}/${type}/`
  );

  if (!fs.existsSync(logDir)) {
    fs.mkdirSync(logDir, { recursive: true });
  }

  return createLogger({
    level: "info",
    format: combine(
      timestamp(),
      errors({ stack: true }),
      json(),
      printf(
        (info) =>
          `${JSON.stringify({
            Level: info.level,
            Time: moment(info.timestamp).format("DD-MM-YYYY hh:mm:ss a"),
            ...info.message,
          })}`
      )
    ),

    transports: [
      new transports.File({
        filename: logDir + module + ".log",
      }),
    ],
  });
};

module.exports = {
  apiLoggerHandler,
};
