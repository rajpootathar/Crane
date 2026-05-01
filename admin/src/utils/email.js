// const sgMail = require('@sendgrid/mail');
// const config = require('../config');

// sgMail.setApiKey(config.SENDGRID_API_KEY);

// module.exports.SendMail = (to, subject, text, html) => {
// 	return new Promise((resolve, reject) => {
// 		const msg = {
// 			to: to,
// 			from: config.SENDGRID_SENDER_EMAIL,
// 			subject: subject,
// 			text: text ? text : 'Email',
// 			html: html ? html : '',
// 		};
// 		sgMail
// 			.send(msg)
// 			.then(() => {
// 				resolve(true);
// 			})
// 			.catch((error) => {
// 				reject(error);
// 			});
// 	});
// };

require("dotenv").config();
const config = require("../config");

const nodemailer = require("nodemailer");
let transporter = nodemailer.createTransport({
  host: "smtp.office365.com",
  port: 587,
  secure: false,
  auth: {
	user: config.MAILER_USERNAME,
	pass: config.MAILER_PASSWORD,
  },
});

module.exports.SendMail = (to, subject, text, html) => {
  return new Promise((resolve, reject) => {
	let mailOptions = {
	  from: `"OneVibe" <${config.MAILER_USERNAME}>`, 
	  to: to, 
	  subject: subject, 
	  ...(text && text),
	  html: html ? html : "",
	};

	transporter.sendMail(mailOptions, (error, info) => {
	  if (error) {
		return reject(error);
	  }
	  resolve(true);
	});
  });
};