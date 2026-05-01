const bcrypt = require("bcryptjs");
var ejs = require("ejs");
var fs = require("fs");
var path = require("path");

module.exports.FormateData = (data) => {
  if (data) {
    return { success: true, data };
  } else {
    throw new Error({ success: false, message: "Data Not found!" });
  }
};

module.exports.GenerateOTP = () => {
  return Math.floor(100000 + Math.random() * 900000);
};

(module.exports.GenerateSalt = async () => {
  return await bcrypt.genSalt();
}),
  (module.exports.GeneratePassword = async (password, salt) => {
    return await bcrypt.hash(password, salt);
  });

exports.renderHTML = function (file, object) {
  var templateString = fs.readFileSync(
    path.join(__dirname + "/../templates/email/" + file),
    "utf-8"
  );
  return ejs.render(templateString, object);
};
