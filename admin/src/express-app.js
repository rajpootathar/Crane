const express = require("express");
const cors = require("cors");
const compression = require("compression");

const AdminUserRoute = require("./routes/admin-user.routes");
const UsersRoute = require("./routes/users-routes");
const PostsRoute = require("./routes/posts.routes");
const ReelsRoute = require("./routes/reels.routes");
const DashboardRoute = require("./routes/dashboard.routes")
const StoriesRoute = require("./routes/stories.routes");
const HashtagsRoute = require("./routes/hashtags.routes");
const HandleErrors = require("./utils/error-handler");

module.exports = async (app) => {
  app.use(express.json({ limit: "1mb" }));
  app.use(express.urlencoded({ extended: true, limit: "1mb" }));
  app.use(cors());
  app.use(compression());
  app.use(express.static(__dirname + "/public"));

  app.get("/healthprobe", (req, res) => {
    res.status(200).send("OK");
  });

  app.use("/", AdminUserRoute);
  app.use("/users", UsersRoute);
  app.use("/posts", PostsRoute);
  app.use("/reels", ReelsRoute);
  app.use("/stories", StoriesRoute)
  app.use("/hashtags", HashtagsRoute)
  app.use("/dashboard", DashboardRoute)

  app.use(HandleErrors);
};
