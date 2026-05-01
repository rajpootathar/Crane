const { 
  User, 
  Reaction, 
  ReelReaction, 
  UserDetails, 
  Post, 
  Comment, 
  Reel, 
  ReelComment, 
  PostReport, 
  UserReport, 
  ProfilePicture
} = require("../../../shared/src/models");
const { Op } = require("sequelize");

class DashBoardService {
  constructor() {
    this.models = { User, Reaction, ReelReaction, UserDetails, Post, Comment, Reel, ReelComment, PostReport, UserReport };
  }

  parseDateFilter(startDate, endDate) {
    const parsedStartDate = startDate 
      ? new Date(`${startDate.split('-').reverse().join('-')}T00:00:00`) 
      : null;
    
    const parsedEndDate = endDate 
      ? new Date(`${endDate.split('-').reverse().join('-')}T23:59:59`) 
      : null;
  
    return parsedStartDate && parsedEndDate
      ? { createdAt: { [Op.between]: [parsedStartDate, parsedEndDate] } }
      : {};
  }  

  async getUserMetrics(filter) {
    try {
      const dateFilter = this.parseDateFilter(filter.startDate, filter.endDate);

      const [totalUsers, activeUsers, newUserRegistrations, verifiedUsers, pendingVerification, reportedUsers] = await Promise.all([
        this.models.User.count(),
        this.models.User.count({ where: { ...dateFilter } }),
        this.models.User.count({ where: { ...dateFilter } }),
        this.models.UserDetails.count({ where: { verified: true, ...dateFilter } }),
        this.models.UserDetails.count({ where: { verified: false, ...dateFilter } }),
        this.models.UserReport.count({ where: { ...dateFilter } }),
      ]);

      return {
        totalUsers,
        activeUsers,
        newUserRegistrations,
        verifiedUsers,
        pendingVerification,
        reportedUsers,
      };
    } catch (error) {
      throw new Error(`Error in getUserMetrics: ${error.message}`);
    }
  }

  async getContentMetrics(filter) {
    try {
      const dateFilter = this.parseDateFilter(filter.startDate, filter.endDate);

      const [totalPhotos, totalVideos, postComments, reelComments, postLikes, reelLikes, postReports] = await Promise.all([
        this.models.Post.count({ where: { ...dateFilter } }),
        this.models.Reel.count({ where: { ...dateFilter } }),
        this.models.Comment.count({ where: { ...dateFilter } }),
        this.models.ReelComment.count({ where: { ...dateFilter } }),
        this.models.Reaction.count({ where: { ...dateFilter } }),
        this.models.ReelReaction.count({ where: { ...dateFilter } }),
        this.models.PostReport.count({ where: { ...dateFilter,type: { [Op.ne]: 'story' }  } }),
      ]);

      return {
        totalPosts: totalPhotos + totalVideos,
        totalPhotos,
        totalVideos,
        totalComments: postComments + reelComments,
        totalLikes: postLikes + reelLikes,
        postReports,
      };
    } catch (error) {
      throw new Error(`Error in getContentMetrics: ${error.message}`);
    }
  }

  async recentUserList(filter) {
    try {
      const { startDate, endDate, page = 1, pageSize = 10 } = filter;
      const dateFilter = this.parseDateFilter(startDate, endDate);

      const result = await this.models.UserDetails.findAndCountAll({
        attributes: [
          "id",
          "userId",
          "userName",
          "fullName",
          "profilePicture",
          "isPrivate",
          "verified",
          "user_id",
        ],
        include: [
          {
            model: ProfilePicture,
            required: false,
            as: "profilePictureDetails",
            attributes: ["id", "filePath"],
          },
          {
            model: this.models.User,
            as: "user",
            attributes: ["id", "email", "status", "isTermsAgreed", "isDeleted"],
          },
        ],
        where: { ...dateFilter },
        limit: pageSize,
        offset: (page - 1) * pageSize,
        order: [["createdAt", "ASC"]],
      });

      return { rows: result.rows, count: result.count };
    } catch (error) {
      throw new Error(`Error in recentUserList: ${error.message}`);
    }
  }

  async getUploadedPost(filter) {
    const { startDate, endDate } = filter;
    const parsedStartDate = startDate ? new Date(startDate.split('-').reverse().join('-')) : null;
    const parsedEndDate = endDate ? new Date(endDate.split('-').reverse().join('-')) : null;
  
    const intervals = this._getDateIntervals(parsedStartDate, parsedEndDate);
    const results = [];
    let totalPosts = 0;
    let totalReels = 0;
  
    for (const interval of intervals) {
      const postCount = await this.models.Post.count({
        where: {
          createdAt: {
            [Op.between]: [interval.start, interval.end],
          },
        },
      });
  
      const reelCount = await this.models.Reel.count({
        where: {
          createdAt: {
            [Op.between]: [interval.start, interval.end],
          },
        },
      });
  
      results.push({
        label: interval.label,
        postCount,
        reelCount,
        totalCount: postCount + reelCount,
      });
  
      totalPosts += postCount;
      totalReels += reelCount;
    }
  
    return {
      intervals: results,
      summary: {
        totalPosts,
        totalReels,
        totalUploaded: totalPosts + totalReels,
      },
    };
  }

  async getUserRegistration(filter) {
    const { startDate, endDate } = filter;
    const parsedStartDate = startDate ? new Date(startDate.split('-').reverse().join('-')) : null;
    const parsedEndDate = endDate ? new Date(endDate.split('-').reverse().join('-')) : null;
    const intervals = this._getDateIntervals(parsedStartDate, parsedEndDate);
  
    const results = [];
    let totalUsers = 0;

    for (const interval of intervals) {
      const userCount = await this.models.User.count({
        where: {
          createdAt: {
            [Op.between]: [interval.start, interval.end],
          },
        },
      });
  
      results.push({
        label: interval.label,
        userCount,
      });
  
      totalUsers += userCount;
    }
  
    return {
      intervals: results,
      summary: {
        totalUsers,
      },
    };
  }
  

  _getDateIntervals (start, end) {
    const startDate = new Date(start);
    const endDate = new Date(end);
    const diffDays = Math.ceil((endDate - startDate) / (1000 * 60 * 60 * 24));
    const intervals = [];
  
    if (diffDays <= 7) {
      // Daily intervals
      for (let i = 0; i <= diffDays; i++) {
        const date = new Date(startDate);
        date.setDate(startDate.getDate() + i);
        intervals.push({
          label: date.toLocaleDateString('en-US', { weekday: 'short' }),
          start: new Date(date.setHours(0, 0, 0, 0)),
          end: new Date(date.setHours(23, 59, 59, 999)),
        });
      }
    } else if (diffDays <= 31) {
      // Weekly intervals
      let current = new Date(startDate);
      while (current < endDate) {
        const weekStart = new Date(current);
        const weekEnd = new Date(current);
        weekEnd.setDate(weekStart.getDate() + 6);
        intervals.push({
          label: `Week ${intervals.length + 1}`,
          start: new Date(weekStart.setHours(0, 0, 0, 0)),
          end: new Date(weekEnd.setHours(23, 59, 59, 999)),
        });
        current.setDate(current.getDate() + 7);
      }
    } else {
      // Monthly intervals
      let current = new Date(startDate);
      while (current <= endDate) {
        const monthStart = new Date(current);
        const monthEnd = new Date(current.getFullYear(), current.getMonth() + 1, 0);
        intervals.push({
          label: `${monthStart.toLocaleString('default', { month: 'short' })}`,
          start: new Date(monthStart.setHours(0, 0, 0, 0)),
          end: new Date(monthEnd.setHours(23, 59, 59, 999)),
        });
        current.setMonth(current.getMonth() + 1);
      }
    }
  
    return intervals;
  };
  
  
}

module.exports = DashBoardService;
